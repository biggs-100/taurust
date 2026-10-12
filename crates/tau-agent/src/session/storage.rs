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

        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        refuse_symlinks(&mut options);

        let file = options.open(&self.lock_path)?;

        if exclusive {
            FileExt::lock_exclusive(&file)?;
        } else {
            FileExt::lock_shared(&file)?;
        }

        // Applied to the open handle, never to the path: a path-based chmod
        // follows a symlink, which is how someone with write access to the session
        // directory would make this process chmod a file of their choosing.
        make_owner_only(&file);
        Ok(LockGuard { file })
    }

    fn read_unlocked(&self) -> Result<Vec<SessionEntry>, SessionStorageError> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(entries_from_json_lines(
                complete_entries(&contents).split('\n'),
            )?),
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
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            owner_only_mode(&mut options);
            let mut file = options.open(&unique)?;
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

    /// Truncate an interrupted final line so the next append starts a clean one.
    ///
    /// Without this the next append would concatenate onto the torn line and put
    /// malformed content in the middle of the transcript, where it is real
    /// corruption rather than a recoverable tail.
    fn repair_torn_tail(&self) -> Result<(), SessionStorageError> {
        let Ok(contents) = std::fs::read_to_string(&self.path) else {
            return Ok(());
        };
        if contents.is_empty() || contents.ends_with('\n') {
            return Ok(());
        }

        let keep = contents.rfind('\n').map_or(0, |index| index + 1);
        let mut options = OpenOptions::new();
        options.write(true);
        refuse_symlinks(&mut options);

        let file = options.open(&self.path)?;
        file.set_len(keep as u64)?;
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
        self.repair_torn_tail()?;

        let mut options = OpenOptions::new();
        options.create(true).append(true);
        refuse_symlinks(&mut options);
        owner_only_mode(&mut options);

        let mut file = options.open(&self.path)?;
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
        // A single append can leave a torn tail behind, and a batch has to clear
        // it too: otherwise the first batched entry would be concatenated onto the
        // partial line, and the newline the batch supplies would hide the damage
        // from the reader's torn-tail recovery.
        self.repair_torn_tail()?;

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

/// Refuse to open a symlink in place of a session file.
///
/// Without this, someone with write access to the session directory can leave a
/// symlink where the transcript or the lock file belongs, and this process will
/// write session data into a file of their choosing, or lock and chmod it.
fn refuse_symlinks(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: open the reparse point itself, not the
        // thing it points at.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = options;
    }
}

/// Create a session file with the mode `mkstemp` would use.
///
/// The reference gets owner-only files for free from `mkstemp`. `File::create`
/// leaves the mode to the umask, which on a typical machine makes a transcript
/// and its temporary file readable by every user of the machine.
fn owner_only_mode(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    {
        let _ = options;
    }
}

/// Restrict an already-open file to its owner, without touching any path.
fn make_owner_only(file: &File) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
}

/// Drop an unterminated final segment.
///
/// Every entry ends with a newline, so a final segment without one is a write
/// that was interrupted, not an entry. Discarding it keeps a torn write from
/// making the whole session unreadable. A malformed line that *is* terminated is
/// still reported, because that is real corruption.
fn complete_entries(contents: &str) -> &str {
    if contents.is_empty() || contents.ends_with('\n') {
        return contents;
    }
    match contents.rfind('\n') {
        Some(index) => &contents[..=index],
        None => "",
    }
}
