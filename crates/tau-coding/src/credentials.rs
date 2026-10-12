//! Local credential storage for Tau provider credentials.
//!
//! Ported from tau's `tau_coding/credentials.py`: a small JSON file under Tau
//! home holding API keys, plain string credentials and refreshable OAuth
//! credentials, read and written with user-only permissions.
//!
//! **Deviations from the reference, deliberate.**
//!
//! The reference resolves its default path from a live `TauPaths()` (which reads
//! `os.environ`). Reading configuration goes through an [`EnvSource`] here for
//! the same reason as the provider configuration, so the caller supplies either
//! an explicit path ([`FileCredentialStore::new`]) or a resolved
//! [`TauPaths`] ([`credentials_path`]).
//!
//! `_validate_oauth_metadata` checks that metadata keys are strings and that
//! values are JSON-compatible. Both checks are unrepresentable here: the field is
//! a `BTreeMap<String, JsonValue>`, which cannot hold a non-string key or a
//! non-JSON value, so there is nothing left to reject.
//!
//! A JSON parse failure or an I/O failure is reported as a
//! [`CredentialStoreError`]. The reference lets `json.JSONDecodeError` and
//! `OSError` escape; folding them into the one error type keeps a caller from
//! having to know which layer refused the file.
//!
//! File permissions are narrowed to `0o600` on Unix only. Windows has no POSIX
//! mode to set, so the reference's `chmod` has no counterpart there.
//!
//! `expires` is an `i64`. Python integers are arbitrary precision, so a stored
//! expiry beyond `i64` would load in the reference and is rejected here as not a
//! positive integer.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Map;
use tau_protocol::JsonValue;

use crate::paths::TauPaths;

/// The file name of the credential store under Tau home.
const CREDENTIALS_FILE_NAME: &str = "credentials.json";

/// The permission the credential file is narrowed to on Unix.
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;

/// A credential store that could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialStoreError {
    message: String,
}

impl CredentialStoreError {
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

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CredentialStoreError {}

impl From<std::io::Error> for CredentialStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for CredentialStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// A refreshable OAuth credential persisted under Tau home.
///
/// `account_id` stays optional so legacy OpenAI Codex credentials load
/// unchanged while device-code providers can persist only the metadata they
/// actually receive. Provider-specific, non-secret values live in `metadata`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OAuthCredential {
    /// The bearer token to present to the provider.
    pub access: String,
    /// The token used to obtain a new access token.
    pub refresh: String,
    /// The expiry as a Unix timestamp.
    pub expires: i64,
    /// The account the credential belongs to, when the provider reports one.
    pub account_id: Option<String>,
    /// Provider-specific, non-secret values.
    pub metadata: BTreeMap<String, JsonValue>,
}

impl OAuthCredential {
    /// Serialize this OAuth credential to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        let mut object = Map::new();
        object.insert("type".to_string(), JsonValue::from("oauth"));
        object.insert("access".to_string(), JsonValue::from(self.access.clone()));
        object.insert("refresh".to_string(), JsonValue::from(self.refresh.clone()));
        object.insert("expires".to_string(), JsonValue::from(self.expires));
        if let Some(account_id) = &self.account_id {
            object.insert(
                "account_id".to_string(),
                JsonValue::from(account_id.clone()),
            );
        }
        if !self.metadata.is_empty() {
            object.insert(
                "metadata".to_string(),
                JsonValue::Object(
                    self.metadata
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                ),
            );
        }
        JsonValue::Object(object)
    }
}

/// An API-key credential persisted under Tau home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyCredential {
    /// The key.
    pub key: String,
}

impl ApiKeyCredential {
    /// Serialize this API-key credential to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        let mut object = Map::new();
        object.insert("type".to_string(), JsonValue::from("api_key"));
        object.insert("key".to_string(), JsonValue::from(self.key.clone()));
        JsonValue::Object(object)
    }
}

/// A credential as it is stored in the file.
#[derive(Debug, Clone, PartialEq)]
enum StoredCredential {
    /// A credential written by an older Tau as a bare string.
    Plain(String),
    /// An API-key credential.
    ApiKey(ApiKeyCredential),
    /// A refreshable OAuth credential.
    OAuth(OAuthCredential),
}

/// Mutable credential-store operations used by setup integrations.
pub trait CredentialStore {
    /// Return a stored API-key credential value by name.
    fn get(&self, name: &str) -> Result<Option<String>, CredentialStoreError>;

    /// Store an API-key credential value by name.
    fn set(&self, name: &str, value: &str) -> Result<(), CredentialStoreError>;

    /// Delete a stored credential value by name.
    fn delete(&self, name: &str) -> Result<(), CredentialStoreError>;

    /// Return credential names without exposing their values.
    fn names(&self, prefix: Option<&str>) -> Result<Vec<String>, CredentialStoreError>;
}

/// A small JSON-backed provider credential store under Tau home.
#[derive(Debug, Clone)]
pub struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    /// A store backed by `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// A store at the default path for `paths`.
    pub fn for_paths(paths: &TauPaths) -> Self {
        Self::new(credentials_path(paths))
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return a stored API-key credential value by name.
    pub fn get(&self, name: &str) -> Result<Option<String>, CredentialStoreError> {
        match self.load()?.get(name) {
            Some(StoredCredential::Plain(value)) => Ok(Some(value.clone())),
            Some(StoredCredential::ApiKey(credential)) => Ok(Some(credential.key.clone())),
            _ => Ok(None),
        }
    }

    /// Store an API-key credential value by name.
    pub fn set(&self, name: &str, value: &str) -> Result<(), CredentialStoreError> {
        let name = validate_credential_name(name)?;
        let value = value.trim();
        if value.is_empty() {
            return Err(CredentialStoreError::new(
                "Credential value must not be empty",
            ));
        }

        let mut data = self.load()?;
        data.insert(name, StoredCredential::Plain(value.to_string()));
        self.save(&data)
    }

    /// Store an API-key credential value by name.
    pub fn set_api_key(&self, name: &str, value: &str) -> Result<(), CredentialStoreError> {
        self.set(name, value)
    }

    /// Return a stored OAuth credential by name.
    pub fn get_oauth(&self, name: &str) -> Result<Option<OAuthCredential>, CredentialStoreError> {
        match self.load()?.get(name) {
            Some(StoredCredential::OAuth(credential)) => Ok(Some(credential.clone())),
            _ => Ok(None),
        }
    }

    /// Store a refreshable OAuth credential by name.
    pub fn set_oauth(
        &self,
        name: &str,
        credential: &OAuthCredential,
    ) -> Result<(), CredentialStoreError> {
        let name = validate_credential_name(name)?;
        validate_oauth_credential(credential)?;

        let mut data = self.load()?;
        data.insert(name, StoredCredential::OAuth(credential.clone()));
        self.save(&data)
    }

    /// Delete a stored credential value by name.
    pub fn delete(&self, name: &str) -> Result<(), CredentialStoreError> {
        let mut data = self.load()?;
        data.remove(name);
        self.save(&data)
    }

    /// Return credential names without exposing their values.
    pub fn names(&self, prefix: Option<&str>) -> Result<Vec<String>, CredentialStoreError> {
        let names: Vec<String> = self.load()?.into_keys().collect();
        Ok(match prefix {
            None => names,
            Some(prefix) => names
                .into_iter()
                .filter(|name| name.starts_with(prefix))
                .collect(),
        })
    }

    fn load(&self) -> Result<BTreeMap<String, StoredCredential>, CredentialStoreError> {
        if !self.path.exists() {
            return Ok(BTreeMap::new());
        }

        let raw: JsonValue = serde_json::from_str(&std::fs::read_to_string(&self.path)?)?;
        let JsonValue::Object(object) = raw else {
            return Err(CredentialStoreError::new(
                "Tau credentials must be a JSON object",
            ));
        };

        let mut credentials = BTreeMap::new();
        for (name, value) in object {
            credentials.insert(name, credential_from_json(&value)?);
        }
        Ok(credentials)
    }

    fn save(&self, data: &BTreeMap<String, StoredCredential>) -> Result<(), CredentialStoreError> {
        let parent = self.path.parent().ok_or_else(|| {
            CredentialStoreError::new("The credential store path has no parent directory")
        })?;
        std::fs::create_dir_all(parent)?;

        let object: Map<String, JsonValue> = data
            .iter()
            .map(|(name, value)| (name.clone(), credential_to_json(value)))
            .collect();
        let content = format!(
            "{}\n",
            serde_json::to_string_pretty(&JsonValue::Object(object))?
        );

        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut temporary = tempfile::Builder::new()
            .prefix(&format!(".{file_name}."))
            .tempfile_in(parent)
            .map_err(io_error)?;
        temporary.write_all(content.as_bytes()).map_err(io_error)?;
        temporary.flush().map_err(io_error)?;
        narrow_permissions(temporary.path())?;
        temporary
            .persist(&self.path)
            .map_err(|error| CredentialStoreError::new(error.error.to_string()))?;
        narrow_permissions(&self.path)
    }
}

impl CredentialStore for FileCredentialStore {
    fn get(&self, name: &str) -> Result<Option<String>, CredentialStoreError> {
        Self::get(self, name)
    }

    fn set(&self, name: &str, value: &str) -> Result<(), CredentialStoreError> {
        Self::set(self, name, value)
    }

    fn delete(&self, name: &str) -> Result<(), CredentialStoreError> {
        Self::delete(self, name)
    }

    fn names(&self, prefix: Option<&str>) -> Result<Vec<String>, CredentialStoreError> {
        Self::names(self, prefix)
    }
}

/// Return Tau's local provider credential path.
pub fn credentials_path(paths: &TauPaths) -> PathBuf {
    paths.home().join(CREDENTIALS_FILE_NAME)
}

fn validate_credential_name(name: &str) -> Result<String, CredentialStoreError> {
    let normalized = name.trim();
    if normalized.is_empty() {
        return Err(CredentialStoreError::new(
            "Credential name must not be empty",
        ));
    }
    Ok(normalized.to_string())
}

fn validate_oauth_credential(credential: &OAuthCredential) -> Result<(), CredentialStoreError> {
    if credential.access.trim().is_empty() {
        return Err(CredentialStoreError::new(
            "OAuth access token must not be empty",
        ));
    }
    if credential.refresh.trim().is_empty() {
        return Err(CredentialStoreError::new(
            "OAuth refresh token must not be empty",
        ));
    }
    if credential
        .account_id
        .as_ref()
        .is_some_and(|account_id| account_id.trim().is_empty())
    {
        return Err(CredentialStoreError::new(
            "OAuth account id must not be empty",
        ));
    }
    if credential.expires <= 0 {
        return Err(CredentialStoreError::new(
            "OAuth expiry must be greater than 0",
        ));
    }
    Ok(())
}

fn credential_from_json(value: &JsonValue) -> Result<StoredCredential, CredentialStoreError> {
    if let JsonValue::String(text) = value {
        return Ok(StoredCredential::Plain(text.clone()));
    }

    let JsonValue::Object(object) = value else {
        return Err(CredentialStoreError::new(
            "Tau credential values must be strings or objects",
        ));
    };

    let credential_type = object.get("type").and_then(JsonValue::as_str);
    match credential_type {
        Some("api_key") => Ok(StoredCredential::ApiKey(ApiKeyCredential {
            key: string_field(object, "key", "api_key")?,
        })),
        Some("oauth") => {
            let expires = match object.get("expires") {
                Some(JsonValue::Number(number)) => match number.as_i64() {
                    Some(expires) if expires > 0 => expires,
                    _ => return Err(oauth_expires_error()),
                },
                _ => return Err(oauth_expires_error()),
            };
            let account_id = match object.get("account_id") {
                None | Some(JsonValue::Null) => None,
                Some(JsonValue::String(account_id)) if !account_id.trim().is_empty() => {
                    Some(account_id.clone())
                }
                _ => {
                    return Err(CredentialStoreError::new(
                        "Tau oauth credential account_id must be a non-empty string",
                    ));
                }
            };
            let metadata = match object.get("metadata") {
                None => BTreeMap::new(),
                Some(JsonValue::Object(metadata)) => metadata
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                _ => {
                    return Err(CredentialStoreError::new(
                        "Tau oauth credential metadata must be an object",
                    ));
                }
            };

            Ok(StoredCredential::OAuth(OAuthCredential {
                access: string_field(object, "access", "oauth")?,
                refresh: string_field(object, "refresh", "oauth")?,
                expires,
                account_id,
                metadata,
            }))
        }
        _ => Err(CredentialStoreError::new(
            "Tau credential object type must be api_key or oauth",
        )),
    }
}

fn oauth_expires_error() -> CredentialStoreError {
    CredentialStoreError::new("Tau oauth credential expires must be a positive integer")
}

fn credential_to_json(value: &StoredCredential) -> JsonValue {
    match value {
        StoredCredential::Plain(text) => JsonValue::String(text.clone()),
        StoredCredential::ApiKey(credential) => credential.to_json(),
        StoredCredential::OAuth(credential) => credential.to_json(),
    }
}

fn string_field(
    object: &Map<String, JsonValue>,
    field_name: &str,
    credential_type: &str,
) -> Result<String, CredentialStoreError> {
    match object.get(field_name) {
        Some(JsonValue::String(text)) if !text.trim().is_empty() => Ok(text.trim().to_string()),
        _ => Err(CredentialStoreError::new(format!(
            "Tau {credential_type} credential field must be a non-empty string: {field_name}"
        ))),
    }
}

fn io_error(error: std::io::Error) -> CredentialStoreError {
    CredentialStoreError::new(error.to_string())
}

#[cfg(unix)]
fn narrow_permissions(path: &Path) -> Result<(), CredentialStoreError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(io_error)
}

#[cfg(not(unix))]
fn narrow_permissions(_path: &Path) -> Result<(), CredentialStoreError> {
    Ok(())
}
