//! Canonical filesystem paths for user and project data.
//!
//! Ported from tau's `tau_coding/paths.py`. Durable application data lives under
//! a Tau home (`~/.tau` by default, overridable with `TAU_HOME`) while project
//! resources are loaded from the active working directory.
//!
//! **Two deviations from the reference, deliberate.**
//!
//! The reference reads `os.environ` and its tests monkeypatch it. Reading goes
//! through an [`EnvSource`] here, for the same reason as the provider
//! configuration: Rust marks environment mutation as `unsafe` and mutating
//! process state from tests is a data race by construction.
//!
//! The reference resolves a project directory with `Path.resolve()`, which
//! follows symlinks. This uses `std::path::absolute`, which does not. On Windows
//! `canonicalize` would also prepend a `\\?\` prefix that would land in the slug
//! and the digest, so the non-canonical form is the portable choice. A symlinked
//! working directory therefore gets a different session directory name than the
//! Python implementation would give it; the name stays stable and readable,
//! which is what it is for.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use tau_ai::env::{EnvSource, ProcessEnv};

/// The environment variable that overrides Tau's home directory.
pub const TAU_HOME_VAR: &str = "TAU_HOME";

/// How much of the path digest is kept in a session directory name.
const DIGEST_LENGTH: usize = 6;

/// The longest slug a session directory name carries.
const SLUG_MAX_LENGTH: usize = 72;

/// A path that could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError {
    message: String,
}

impl PathError {
    /// An error described by `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PathError {}

/// Resolve a Tau home from a `TAU_HOME` value and a user home directory.
///
/// An unset or empty value falls back to `<home>/.tau`. A value beginning with
/// `~` is expanded against `home`, and the result must be absolute.
pub fn tau_home_from(env: &dyn EnvSource, home: Option<&Path>) -> Result<PathBuf, PathError> {
    let Some(value) = env.get(TAU_HOME_VAR) else {
        return Ok(resolve_user_home(home)?.join(".tau"));
    };
    if value.is_empty() {
        return Ok(resolve_user_home(home)?.join(".tau"));
    }

    let expanded = expand_tilde(&value, home)?;
    if !expanded.is_absolute() {
        return Err(PathError::new(
            "TAU_HOME must be an absolute path after '~' expansion",
        ));
    }
    Ok(expanded)
}

fn resolve_user_home(home: Option<&Path>) -> Result<PathBuf, PathError> {
    home.map(Path::to_path_buf)
        .or_else(dirs::home_dir)
        .ok_or_else(|| PathError::new("the user home directory could not be determined"))
}

/// Expand a leading `~` or `~/`, the only forms the reference supports.
fn expand_tilde(value: &str, home: Option<&Path>) -> Result<PathBuf, PathError> {
    let Some(rest) = value.strip_prefix('~') else {
        return Ok(PathBuf::from(value));
    };
    if !rest.is_empty() && !rest.starts_with('/') && !rest.starts_with('\\') {
        return Ok(PathBuf::from(value));
    }

    let Some(home) = home.map(Path::to_path_buf).or_else(dirs::home_dir) else {
        return Err(PathError::new(
            "TAU_HOME could not expand '~'; use an absolute path or an existing user home",
        ));
    };

    Ok(home.join(rest.trim_start_matches(['/', '\\'])))
}

/// The resolved Tau filesystem locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TauPaths {
    home: PathBuf,
    agents_home: PathBuf,
}

impl TauPaths {
    /// Paths with an explicit home and agents home.
    pub fn new(home: impl Into<PathBuf>, agents_home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            agents_home: agents_home.into(),
        }
    }

    /// Paths resolved from an environment and a user home directory.
    pub fn from_environment(
        env: &dyn EnvSource,
        user_home: Option<&Path>,
    ) -> Result<Self, PathError> {
        let home = tau_home_from(env, user_home)?;
        let agents_home = resolve_user_home(user_home)?.join(".agents");
        Ok(Self { home, agents_home })
    }

    /// Paths resolved from the real process environment.
    pub fn process() -> Result<Self, PathError> {
        Self::from_environment(&ProcessEnv, None)
    }

    /// The Tau home directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The user-level `.agents` directory.
    pub fn agents_home(&self) -> &Path {
        &self.agents_home
    }

    /// The user-level session directory.
    pub fn sessions_dir(&self) -> PathBuf {
        self.home.join("sessions")
    }

    /// The user-level diagnostic log directory.
    pub fn logs_dir(&self) -> PathBuf {
        self.home.join("logs")
    }

    /// The JSONL diagnostic log for agent-call failures.
    pub fn agent_calls_log_path(&self) -> PathBuf {
        self.logs_dir().join("agent-calls.jsonl")
    }

    /// The persisted remote model-catalog cache.
    pub fn models_store_path(&self) -> PathBuf {
        self.home.join("models-store.json")
    }

    /// The latest released Codex-version cache.
    pub fn codex_version_store_path(&self) -> PathBuf {
        self.home.join("codex-version-store.json")
    }

    /// The account-scoped Codex model-catalog cache.
    pub fn codex_models_store_path(&self) -> PathBuf {
        self.home.join("codex-models-store.json")
    }

    /// The user-level state directory owned by built-in extensions.
    pub fn extension_state_dir(&self) -> PathBuf {
        self.home.join("state").join("extensions")
    }

    /// The built-in llama.cpp integration state.
    pub fn llama_cpp_state_path(&self) -> PathBuf {
        self.extension_state_dir().join("llama.cpp.json")
    }

    /// The user-level skills directory.
    pub fn user_skills_dir(&self) -> PathBuf {
        self.home.join("skills")
    }

    /// The user-level prompt templates directory.
    pub fn user_prompts_dir(&self) -> PathBuf {
        self.home.join("prompts")
    }

    /// The user-level TUI themes directory.
    pub fn user_themes_dir(&self) -> PathBuf {
        self.home.join("themes")
    }

    /// The user-level extension directory.
    pub fn user_extensions_dir(&self) -> PathBuf {
        self.home.join("extensions")
    }

    /// The user-level `.agents/skills` directory.
    pub fn user_agents_skills_dir(&self) -> PathBuf {
        self.agents_home.join("skills")
    }

    /// The user-level `.agents/prompts` directory.
    pub fn user_agents_prompts_dir(&self) -> PathBuf {
        self.agents_home.join("prompts")
    }

    /// The project-local Tau resource directory.
    pub fn project_tau_dir(&self, cwd: &Path) -> PathBuf {
        cwd.join(".tau")
    }

    /// The project-local `.agents` resource directory.
    pub fn project_agents_dir(&self, cwd: &Path) -> PathBuf {
        cwd.join(".agents")
    }

    /// The project-local Tau skills directory.
    pub fn project_skills_dir(&self, cwd: &Path) -> PathBuf {
        self.project_tau_dir(cwd).join("skills")
    }

    /// The project-local Tau prompt templates directory.
    pub fn project_prompts_dir(&self, cwd: &Path) -> PathBuf {
        self.project_tau_dir(cwd).join("prompts")
    }

    /// The project-local Tau TUI themes directory.
    pub fn project_themes_dir(&self, cwd: &Path) -> PathBuf {
        self.project_tau_dir(cwd).join("themes")
    }

    /// The project-local `.agents/skills` directory.
    pub fn project_agents_skills_dir(&self, cwd: &Path) -> PathBuf {
        self.project_agents_dir(cwd).join("skills")
    }

    /// The project-local `.agents/prompts` directory.
    pub fn project_agents_prompts_dir(&self, cwd: &Path) -> PathBuf {
        self.project_agents_dir(cwd).join("prompts")
    }

    /// The user-home session directory that belongs to a project directory.
    pub fn project_session_dir(&self, cwd: &Path) -> PathBuf {
        let resolved = resolve(cwd);
        let normalized = resolved.to_string_lossy();
        let digest = &digest_of(&normalized)[..DIGEST_LENGTH];
        let slug = slugify_path(&resolved, SLUG_MAX_LENGTH);
        let name = if slug.is_empty() {
            "project".to_string()
        } else {
            slug
        };

        self.sessions_dir().join(format!("{name}-{digest}"))
    }

    /// The default JSONL session path for a project directory, with its
    /// directory created.
    pub fn default_session_path(&self, cwd: &Path) -> Result<PathBuf, PathError> {
        let path = self.project_session_dir(cwd).join("default.jsonl");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(path)
    }
}

impl From<std::io::Error> for PathError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// An absolute form of `path` without following symlinks and without requiring
/// it to exist.
fn resolve(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn digest_of(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Render a path as a readable, bounded directory name.
///
/// A path under the user's home starts with `home`, so the name says where the
/// project lives rather than repeating the machine's directory layout. When the
/// whole slug is too long, as many trailing parts as fit are kept, because the
/// end of a path identifies a project better than its beginning.
fn slugify_path(path: &Path, max_length: usize) -> String {
    let mut parts = normal_parts(path);

    if let Some(home) = dirs::home_dir()
        && let Ok(relative) = path.strip_prefix(&home)
    {
        parts = std::iter::once("home".to_string())
            .chain(normal_parts(relative))
            .collect();
    }

    let slug_parts: Vec<String> = parts
        .iter()
        .map(|part| normalize_part(part))
        .filter(|part| !part.is_empty())
        .collect();
    let slug = slug_parts.join("-");

    if slug.chars().count() <= max_length {
        return slug;
    }

    let mut suffix: Vec<&String> = Vec::new();
    let mut length = 0usize;
    for part in slug_parts.iter().rev() {
        let next = length + part.chars().count() + usize::from(!suffix.is_empty());
        if next > max_length {
            break;
        }
        suffix.push(part);
        length = next;
    }

    if suffix.is_empty() {
        let characters: Vec<char> = slug.chars().collect();
        let start = characters.len().saturating_sub(max_length);
        return characters[start..]
            .iter()
            .collect::<String>()
            .trim_matches(['-', '.', '_'])
            .to_string();
    }

    suffix.reverse();
    suffix
        .iter()
        .map(|part| part.as_str())
        .collect::<Vec<_>>()
        .join("-")
}

fn normal_parts(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// Replace every run of characters outside the allowed set with a single `-`,
/// then trim the edges and lowercase.
fn normalize_part(part: &str) -> String {
    let mut normalized = String::with_capacity(part.len());
    let mut in_run = false;

    for character in part.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            normalized.push(character);
            in_run = false;
        } else if !in_run {
            normalized.push('-');
            in_run = true;
        }
    }

    normalized.trim_matches(['.', '-', '_']).to_lowercase()
}
