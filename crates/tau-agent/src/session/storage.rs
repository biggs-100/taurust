//! Append-only session storage.
//!
//! Ported from tau's `tau_agent/session/storage.py`.
//!
//! **Deviation from the reference, deliberate.** The reference's storage methods
//! are `async`, because it takes an asyncio lock. Rust's file locking is
//! blocking, so these are synchronous: marking a blocking call `async` would only
//! make the signature lie. A caller on an async runtime wraps them in
//! `spawn_blocking`, which is where the thread actually belongs.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;

use super::entries::SessionEntry;
use super::jsonl::{SessionJsonlError, entries_from_json_lines, entry_to_json_line};

/// A session store operation that failed.
#[derive(Debug)]
pub enum SessionStorageError {
    /// The file system refused the operation.
    Io(std::io::Error),
    /// A stored entry could not be decoded.
    Jsonl(SessionJsonlError),
}

impl fmt::Display for SessionStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Jsonl(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for SessionStorageError {}

impl From<std::io::Error> for SessionStorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<SessionJsonlError> for SessionStorageError {
    fn from(error: SessionJsonlError) -> Self {
        Self::Jsonl(error)
    }
}

/// Append-only session storage.
///
/// [`SessionStorage::append_batch`] is the durable transaction boundary: every
/// entry in the batch becomes visible, or the previous transcript is untouched.
pub trait SessionStorage {
    /// Append one entry.
    fn append(&self, entry: &SessionEntry) -> Result<(), SessionStorageError>;

    /// Atomically append a complete batch of entries.
    fn append_batch(&self, entries: &[SessionEntry]) -> Result<(), SessionStorageError>;

    /// Read every entry in storage order.
    fn read_all(&self) -> Result<Vec<SessionEntry>, SessionStorageError>;
}

/// Deterministic storage for tests and embedded frontends.
#[derive(Debug, Default)]
pub struct InMemorySessionStorage {
    entries: Mutex<Vec<SessionEntry>>,
}

impl InMemorySessionStorage {
    /// Storage preloaded with `entries`.
    pub fn new(entries: Vec<SessionEntry>) -> Self {
        Self {
            entries: Mutex::new(entries),
        }
    }

    /// A copy of everything stored.
    pub fn entries(&self) -> Vec<SessionEntry> {
        self.entries.lock().expect("the entries lock").clone()
    }
}

impl SessionStorage for InMemorySessionStorage {
    fn append(&self, entry: &SessionEntry) -> Result<(), SessionStorageError> {
        self.entries
            .lock()
            .expect("the entries lock")
            .push(entry.clone());
        Ok(())
    }

    fn append_batch(&self, entries: &[SessionEntry]) -> Result<(), SessionStorageError> {
        self.entries
            .lock()
            .expect("the entries lock")
            .extend_from_slice(entries);
        Ok(())
    }

    fn read_all(&self) -> Result<Vec<SessionEntry>, SessionStorageError> {
        Ok(self.entries())
    }
}

/// Local JSONL storage with a per-session cross-process lock.
///
/// The lock is deliberately separate from the transcript. Readers take a shared
/// lock where the platform provides one; every write takes an exclusive lock. A
/// batch write goes through a same-directory temporary file, is flushed, and is
/// then renamed over the transcript, so a crash can never leave a half-written
/// batch.
#[derive(Debug)]
pub struct JsonlSessionStorage {
    path: PathBuf,
    lock_path: PathBuf,
    temp_path: PathBuf,
}

impl JsonlSessionStorage {
    /// Storage rooted at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();

        Self {
            lock_path: parent.join(format!(".{name}.lock")),
            temp_path: parent.join(format!(".{name}.tmp")),
            path,
        }
    }

    /// The transcript path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn ensure_parent(&self) -> Result<(), SessionStorageError> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Take the session lock, held until the returned guard is dropped.
    fn lock(&self, exclusive: bool) -> Result<LockGuard, SessionStorageError> {
        if let Some(parent) = self.lock_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&self.lock_path)?;

        if exclusive {
            FileExt::lock_exclusive(&file)?;
        } else {
            FileExt::lock_shared(&file)?;
        }

        restrict_to_owner(&self.lock_path);
        Ok(LockGuard { file })
    }

    fn read_unlocked(&self) -> Result<Vec<SessionEntry>, SessionStorageError> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(entries_from_json_lines(contents.split('\n'))?),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }

    /// Write `data` through a temporary file and rename it over the transcript.
    fn atomic_replace(&self, data: &[u8]) -> Result<(), SessionStorageError> {
        let parent = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let unique = parent.join(format!(
            ".{}.{}.tmp",
            self.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            unique_suffix(),
        ));

        let written = (|| -> Result<(), SessionStorageError> {
            let mut file = File::create(&unique)?;
            file.write_all(data)?;
            file.flush()?;
            file.sync_all()?;
            Ok(())
        })();

        if let Err(error) = written {
            let _ = std::fs::remove_file(&unique);
            return Err(error);
        }

        std::fs::rename(&unique, &self.path)?;
        sync_directory(&parent);
        Ok(())
    }

    /// Discard temporary files a crash left behind.
    ///
    /// A fixed temp name makes recovery deterministic; unique ones left by a
    /// process killed during a replacement are never authoritative, because the
    /// rename is atomic.
    fn remove_incomplete_temp(&self) {
        let _ = std::fs::remove_file(&self.temp_path);

        let Some(parent) = self.path.parent() else {
            return;
        };
        let prefix = format!(
            ".{}.",
            self.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        let Ok(entries) = std::fs::read_dir(parent) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && name.ends_with(".tmp") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

impl SessionStorage for JsonlSessionStorage {
    fn append(&self, entry: &SessionEntry) -> Result<(), SessionStorageError> {
        self.ensure_parent()?;
        let _guard = self.lock(true)?;
        self.remove_incomplete_temp();

        let line = entry_to_json_line(entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    }

    fn append_batch(&self, entries: &[SessionEntry]) -> Result<(), SessionStorageError> {
        if entries.is_empty() {
            return Ok(());
        }

        self.ensure_parent()?;
        let mut encoded = String::new();
        for entry in entries {
            encoded.push_str(&entry_to_json_line(entry)?);
        }

        let _guard = self.lock(true)?;
        self.remove_incomplete_temp();

        let mut data = match std::fs::read(&self.path) {
            Ok(previous) => previous,
            Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        data.extend_from_slice(encoded.as_bytes());

        self.atomic_replace(&data)
    }

    fn read_all(&self) -> Result<Vec<SessionEntry>, SessionStorageError> {
        self.ensure_parent()?;

        if self.temp_path.exists() {
            let _guard = self.lock(true)?;
            self.remove_incomplete_temp();
            return self.read_unlocked();
        }

        let _guard = self.lock(false)?;
        self.read_unlocked()
    }
}

/// Releases the session lock when dropped.
struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}-{count}", std::process::id())
}

/// Persist the directory entry where the platform supports it.
fn sync_directory(path: &Path) {
    #[cfg(unix)]
    {
        if let Ok(directory) = File::open(path) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Keep the lock file readable only by its owner, where the platform allows it.
fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}
