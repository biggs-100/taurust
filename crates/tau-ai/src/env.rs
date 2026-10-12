//! Provider configuration read from the environment.
//!
//! Ported from tau's `tau_ai/env.py` for the reading and validation policy. The
//! provider configuration records themselves arrive with the adapters.
//!
//! **Deviation from the reference, deliberate.** tau reads `os.environ` directly,
//! and its tests set variables with a monkeypatch fixture. Rust marks
//! `std::env::set_var` as `unsafe` in edition 2024, which this workspace forbids,
//! and mutating process-wide state from tests is a data race by construction.
//! Reading is expressed through an [`EnvSource`] instead, so a caller can inject
//! configuration and tests need no global state.

use std::collections::BTreeMap;
use std::fmt;

/// The default endpoint for OpenAI-compatible providers.
pub const DEFAULT_OPENAI_COMPATIBLE_BASE_URL: &str = "https://api.openai.com/v1";

/// The default endpoint for Anthropic's Messages API.
pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1";

/// The default request timeout, in seconds.
pub const DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS: f64 = 60.0;

/// The default number of retries after a failed request.
pub const DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES: i64 = 2;

/// The default cap on the exponential retry backoff, in seconds.
pub const DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS: f64 = 1.0;

/// Where configuration values are read from.
pub trait EnvSource {
    /// Return the value of `name`, or `None` when it is unset.
    fn get(&self, name: &str) -> Option<String>;
}

/// The real process environment.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// An in-memory environment, for tests and for injected configuration.
#[derive(Debug, Default, Clone)]
pub struct MapEnv(BTreeMap<String, String>);

impl MapEnv {
    /// An empty environment.
    pub fn new() -> Self {
        Self::default()
    }

    /// An environment with one more value set.
    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.0.insert(name.to_string(), value.to_string());
        self
    }
}

impl EnvSource for MapEnv {
    fn get(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
}

/// A configuration value that is unusable, with the reason a user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    /// An error with the given message.
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

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

/// Read a required value, reporting the variable by name when it is empty.
pub fn required_env(env: &dyn EnvSource, name: &str) -> Result<String, ConfigError> {
    match env.get(name) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(ConfigError::new(format!(
            "Missing required environment variable: {name}"
        ))),
    }
}

/// Read a positive duration in seconds, falling back to `default` when unset.
pub fn timeout_seconds_from_env(
    env: &dyn EnvSource,
    name: &str,
    default: f64,
) -> Result<f64, ConfigError> {
    let Some(raw) = env.get(name) else {
        return Ok(default);
    };

    let parsed = raw
        .parse::<f64>()
        .map_err(|_| ConfigError::new(format!("Environment variable must be a number: {name}")))?;
    if parsed <= 0.0 {
        return Err(ConfigError::new(format!(
            "Environment variable must be greater than 0: {name}"
        )));
    }

    Ok(parsed)
}

/// Read a non-negative integer, falling back to `default` when unset.
pub fn non_negative_int_from_env(
    env: &dyn EnvSource,
    name: &str,
    default: i64,
) -> Result<i64, ConfigError> {
    let Some(raw) = env.get(name) else {
        return Ok(default);
    };

    let parsed = raw.parse::<i64>().map_err(|_| {
        ConfigError::new(format!("Environment variable must be an integer: {name}"))
    })?;
    if parsed < 0 {
        return Err(ConfigError::new(format!(
            "Environment variable must be 0 or greater: {name}"
        )));
    }

    Ok(parsed)
}

/// Read a non-negative number, falling back to `default` when unset.
pub fn non_negative_float_from_env(
    env: &dyn EnvSource,
    name: &str,
    default: f64,
) -> Result<f64, ConfigError> {
    let Some(raw) = env.get(name) else {
        return Ok(default);
    };

    let parsed = raw
        .parse::<f64>()
        .map_err(|_| ConfigError::new(format!("Environment variable must be a number: {name}")))?;
    if parsed < 0.0 {
        return Err(ConfigError::new(format!(
            "Environment variable must be 0 or greater: {name}"
        )));
    }

    Ok(parsed)
}
