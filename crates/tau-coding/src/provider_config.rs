//! Durable provider configuration and the rules that resolve it into a usable
//! provider, model and credential.
//!
//! Ported from tau's `tau_coding/provider_config.py`: the `providers.json`
//! shape, the defaults each provider kind carries, every validation the
//! reference performs on those values, the error messages verbatim, and the
//! precedence between an explicitly configured value, a stored credential and
//! the process environment.
//!
//! **Deviations from the reference, deliberate.**
//!
//! Reading environment values goes through an [`EnvSource`], for the same
//! reason as the provider environment module: Rust marks environment mutation as
//! `unsafe`, and mutating process-wide state from tests is a data race by
//! construction. `MapEnv` replaces the reference's monkeypatching.
//!
//! The reference validates inside a dataclass `__post_init__`, so an invalid
//! value cannot be constructed. Each configuration here carries the same
//! validation in an explicit [`ProviderConfig::validate`] returning a
//! [`ProviderConfigError`]; the JSON readers call it for every record they
//! build, so the observable behaviour of `provider_settings_from_json` and
//! `load_provider_settings` is unchanged.
//!
//! `provider_settings_from_json` takes the effective provider catalog as an
//! argument. The reference reaches for `_effective_provider_configs(paths)`,
//! which merges the built-in catalog with the user's `catalog.toml`; neither
//! `tau-coding::provider_catalog` nor `tau-coding::catalog_loader` is ported yet,
//! and a preferences file is pure overrides that only mean something against a
//! catalog.
//!
//! `load_provider_settings` reads and parses the file, and falls back to the
//! catalog when it is absent. It does not perform the follow-up steps the
//! reference performs: merging catalog definitions back in
//! (`_with_builtin_catalog_models`), appending credentialed built-ins
//! (`_append_catalog_providers`), or migrating the legacy `providers[]` shape and
//! writing it back (`_migrate_legacy_provider_settings`). All three need the
//! catalog, the user catalog file and the OAuth registry.
//!
//! Writing is out of scope: `save_provider_settings`, the backup file, the
//! `set_*`/`toggle_*`/`upsert_*` preference editors and the catalog write-back
//! belong to the setup flows. `ProviderSettings::to_json` is ported because it is
//! the inverse of the reader and the reference's own tests assert its shape.
//!
//! The runtime adapter configuration (`openai_compatible_config_from_provider`,
//! `anthropic_config_from_provider`) is not ported: both return
//! `OpenAICompatibleConfig`/`AnthropicConfig`, types owned by `tau_ai`, which
//! does not define them yet, and `tau-coding` must not redefine a `tau_ai` type.
//! The decisions those functions make are exposed instead as
//! [`api_key_from_provider`], [`openai_compatible_base_url`], [`provider_headers`]
//! and the provider's own numbers, so the credential and base-URL precedence is
//! testable now and the adapter can assemble them later.
//!
//! [`CredentialReader::get_oauth`] returns the stored access token rather than
//! the credential record, and the reference's `oauth_registry` gate is not
//! applied: OAuth flows are out of scope, and a reader that holds no OAuth
//! credential returns nothing either way.
//!
//! `_api_key_from_provider` raises `RuntimeError` for a missing key; it is a
//! [`ProviderConfigError`] here, the module's single error type.
//!
//! The thinking-level vocabulary and level-map precedence are reproduced here
//! because `tau-coding::thinking` is not ported yet and `providers.json` stores
//! and validates those values. Only the vocabulary, [`provider_thinking_levels`]
//! and [`provider_default_thinking_level`] are ported; the session-startup
//! policy (`resolve_startup_thinking_level`), the effort and budget mapping, and
//! the unavailable-reason strings stay with that module.
//!
//! Two reference checks are unrepresentable here. `Provider thinking_levels
//! must be normalized` cannot fire, because [`ThinkingLevel`] can only hold a
//! normalized level, and `Provider thinking_parameter must be reasoning_effort,
//! reasoning.effort, or anthropic.thinking` cannot fire, because
//! [`ThinkingParameter`] is a closed enum.
//!
//! One reference crash is replaced by a reported error: a `thinking_levels` list
//! holding a non-string makes `normalize_thinking_level` call `.strip()` on it,
//! which escapes as an `AttributeError` rather than a `ProviderConfigError`. The
//! list is shape-checked first here, so the caller gets a provider configuration
//! error like any other malformed field.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use serde_json::Map;
use tau_ai::env::{
    DEFAULT_ANTHROPIC_BASE_URL, DEFAULT_OPENAI_COMPATIBLE_BASE_URL,
    DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES, DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS,
    DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS, EnvSource,
};
use tau_protocol::JsonValue;

use crate::credentials::FileCredentialStore;
use crate::paths::TauPaths;

/// The provider Tau selects when no preference says otherwise.
pub const DEFAULT_PROVIDER_NAME: &str = "openai";

/// The default OpenAI model.
pub const DEFAULT_MODEL: &str = "gpt-5.4";

/// The `providers.json` schema version this module reads and writes.
pub const PROVIDER_SETTINGS_SCHEMA_VERSION: i64 = 2;

/// The durable provider settings file name under Tau home.
const PROVIDER_SETTINGS_FILE_NAME: &str = "providers.json";

/// The default endpoint for the OpenAI Codex subscription API.
const DEFAULT_OPENAI_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";

/// The default Anthropic model.
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";

/// The environment variable the OpenAI-compatible default provider reads.
const DEFAULT_OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// The environment variable Anthropic reads.
const DEFAULT_ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// The environment variable OpenAI Codex reads.
const DEFAULT_OPENAI_CODEX_API_KEY_ENV: &str = "OPENAI_CODEX_ACCESS_TOKEN";

/// The environment variable that relocates the OpenAI endpoint.
const OPENAI_BASE_URL_VAR: &str = "OPENAI_BASE_URL";

/// The `type` marker of an OpenAI-compatible provider record.
const OPENAI_COMPATIBLE_TYPE: &str = "openai-compatible";

/// The `type` marker of an Anthropic provider record.
const ANTHROPIC_TYPE: &str = "anthropic";

/// The `type` marker of an OpenAI Codex provider record.
const OPENAI_CODEX_TYPE: &str = "openai-codex";

/// The models the OpenAI Codex subscription provider declares.
const OPENAI_CODEX_MODELS: [&str; 6] = [
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex",
    "gpt-5.3-codex-spark",
    "gpt-5.2",
];

/// A provider configuration that is invalid or unusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigError {
    message: String,
}

impl ProviderConfigError {
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

impl fmt::Display for ProviderConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProviderConfigError {}

/// Credential lookup used while resolving a provider's API key.
pub trait CredentialReader {
    /// The stored API-key credential for `name`, when one exists.
    fn get(&self, name: &str) -> Option<String>;

    /// The stored OAuth access token for `name`, when one exists.
    fn get_oauth(&self, _name: &str) -> Option<String> {
        None
    }
}

impl CredentialReader for FileCredentialStore {
    fn get(&self, name: &str) -> Option<String> {
        self.get(name).ok().flatten()
    }

    fn get_oauth(&self, name: &str) -> Option<String> {
        self.get_oauth(name)
            .ok()
            .flatten()
            .map(|credential| credential.access)
    }
}

/// A Tau thinking mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThinkingLevel {
    /// No reasoning.
    Off,
    /// Very brief reasoning.
    Minimal,
    /// Light reasoning.
    Low,
    /// Moderate reasoning.
    Medium,
    /// Deep reasoning.
    High,
    /// Very deep reasoning.
    Xhigh,
    /// Maximum reasoning.
    Max,
}

impl ThinkingLevel {
    /// Every level, in the order Tau presents them.
    pub const ALL: [ThinkingLevel; 7] = [
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::Xhigh,
        ThinkingLevel::Max,
    ];

    /// The level Tau prefers when nothing else is configured.
    pub const DEFAULT: ThinkingLevel = ThinkingLevel::Medium;

    /// The persisted spelling of this level.
    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingLevel::Off => "off",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::Xhigh => "xhigh",
            ThinkingLevel::Max => "max",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        ThinkingLevel::ALL
            .into_iter()
            .find(|level| level.as_str() == value)
    }
}

impl Default for ThinkingLevel {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The provider field a thinking level is serialized into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingParameter {
    /// `reasoning_effort`.
    ReasoningEffort,
    /// `reasoning.effort`.
    ReasoningDotEffort,
    /// `anthropic.thinking`.
    AnthropicThinking,
}

impl ThinkingParameter {
    /// The persisted spelling of this parameter.
    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingParameter::ReasoningEffort => "reasoning_effort",
            ThinkingParameter::ReasoningDotEffort => "reasoning.effort",
            ThinkingParameter::AnthropicThinking => "anthropic.thinking",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "reasoning_effort" => Some(ThinkingParameter::ReasoningEffort),
            "reasoning.effort" => Some(ThinkingParameter::ReasoningDotEffort),
            "anthropic.thinking" => Some(ThinkingParameter::AnthropicThinking),
            _ => None,
        }
    }
}

fn available_thinking_modes() -> String {
    ThinkingLevel::ALL
        .iter()
        .map(|level| level.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Return a valid Tau thinking level or a user-facing error.
pub fn normalize_thinking_level(value: Option<&str>) -> Result<ThinkingLevel, ProviderConfigError> {
    let Some(value) = value else {
        return Ok(ThinkingLevel::DEFAULT);
    };

    let normalized = value.trim().to_lowercase();
    ThinkingLevel::parse(&normalized).ok_or_else(|| {
        ProviderConfigError::new(format!(
            "Unknown thinking mode: {value}. Available modes: {}",
            available_thinking_modes()
        ))
    })
}

fn normalize_thinking_levels(values: &[String]) -> Result<Vec<ThinkingLevel>, ProviderConfigError> {
    if values.is_empty() {
        return Err(ProviderConfigError::new(format!(
            "Thinking modes must be a non-empty list. Available modes: {}",
            available_thinking_modes()
        )));
    }

    let normalized = values
        .iter()
        .map(|value| normalize_thinking_level(Some(value)))
        .collect::<Result<Vec<_>, _>>()?;
    let unique: BTreeSet<ThinkingLevel> = normalized.iter().copied().collect();
    if unique.len() != normalized.len() {
        return Err(ProviderConfigError::new("Thinking modes must be unique"));
    }
    Ok(normalized)
}

/// The wire protocol a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderApi {
    /// The OpenAI completions API.
    OpenAiCompletions,
    /// The OpenAI responses API.
    OpenAiResponses,
    /// Anthropic's Messages API.
    AnthropicMessages,
    /// The OpenAI Codex responses API.
    OpenAiCodexResponses,
    /// Google's Generative AI API.
    GoogleGenerativeAi,
    /// Mistral's Conversations API.
    MistralConversations,
}

impl ProviderApi {
    /// The persisted spelling of this API.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderApi::OpenAiCompletions => "openai-completions",
            ProviderApi::OpenAiResponses => "openai-responses",
            ProviderApi::AnthropicMessages => "anthropic-messages",
            ProviderApi::OpenAiCodexResponses => "openai-codex-responses",
            ProviderApi::GoogleGenerativeAi => "google-generative-ai",
            ProviderApi::MistralConversations => "mistral-conversations",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "openai-completions" => Some(ProviderApi::OpenAiCompletions),
            "openai-responses" => Some(ProviderApi::OpenAiResponses),
            "anthropic-messages" => Some(ProviderApi::AnthropicMessages),
            "openai-codex-responses" => Some(ProviderApi::OpenAiCodexResponses),
            "google-generative-ai" => Some(ProviderApi::GoogleGenerativeAi),
            "mistral-conversations" => Some(ProviderApi::MistralConversations),
            _ => None,
        }
    }
}

/// The durable kind of a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// An OpenAI-compatible endpoint.
    OpenAiCompatible,
    /// Anthropic's Messages API.
    Anthropic,
    /// The OpenAI Codex subscription.
    OpenAiCodex,
    /// Google's Generative AI API.
    GoogleGenerativeAi,
    /// Mistral's Conversations API.
    MistralConversations,
}

impl ProviderKind {
    /// The persisted spelling of this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::OpenAiCompatible => OPENAI_COMPATIBLE_TYPE,
            ProviderKind::Anthropic => ANTHROPIC_TYPE,
            ProviderKind::OpenAiCodex => OPENAI_CODEX_TYPE,
            ProviderKind::GoogleGenerativeAi => "google-generative-ai",
            ProviderKind::MistralConversations => "mistral-conversations",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            OPENAI_COMPATIBLE_TYPE => Some(ProviderKind::OpenAiCompatible),
            ANTHROPIC_TYPE => Some(ProviderKind::Anthropic),
            OPENAI_CODEX_TYPE => Some(ProviderKind::OpenAiCodex),
            "google-generative-ai" => Some(ProviderKind::GoogleGenerativeAi),
            "mistral-conversations" => Some(ProviderKind::MistralConversations),
            _ => None,
        }
    }

    fn default_api(self) -> ProviderApi {
        match self {
            ProviderKind::Anthropic => ProviderApi::AnthropicMessages,
            ProviderKind::OpenAiCodex => ProviderApi::OpenAiCodexResponses,
            ProviderKind::GoogleGenerativeAi => ProviderApi::GoogleGenerativeAi,
            ProviderKind::MistralConversations => ProviderApi::MistralConversations,
            ProviderKind::OpenAiCompatible => ProviderApi::OpenAiCompletions,
        }
    }
}

/// Model rates that apply up to an optional input-token limit.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCostTier {
    /// The rates applied within this tier.
    pub cost: BTreeMap<String, f64>,
    /// The inclusive input-token limit of this tier.
    pub max_input_tokens: Option<i64>,
}

impl ModelCostTier {
    /// A tier with `cost` and no input limit.
    pub fn new(cost: BTreeMap<String, f64>) -> Self {
        Self {
            cost,
            max_input_tokens: None,
        }
    }

    /// Serialize this tier to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        let mut object: Map<String, JsonValue> = Map::new();
        if let Some(limit) = self.max_input_tokens {
            object.insert("max_input_tokens".to_string(), JsonValue::from(limit));
        }
        for (key, value) in &self.cost {
            object.insert(key.clone(), json_number(*value));
        }
        JsonValue::Object(object)
    }
}

/// Runtime metadata for one configured model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProviderModelMetadata {
    /// The model id the provider expects, when it differs from the key.
    pub name: Option<String>,
    /// The API this model is served by, when it differs from the provider.
    pub api: Option<ProviderApi>,
    /// The endpoint this model is served from, when it differs.
    pub base_url: Option<String>,
    /// Whether the model reasons; `None` means the provider catalog is silent.
    pub reasoning: Option<bool>,
    /// The accepted input kinds, `text` or `image`.
    pub input: Vec<String>,
    /// The model's flat rates.
    pub cost: BTreeMap<String, f64>,
    /// The model's input-size-dependent rates.
    pub cost_tiers: Vec<ModelCostTier>,
    /// The model's context window in tokens.
    pub context_window: Option<i64>,
    /// The model's output token limit.
    pub max_tokens: Option<i64>,
    /// Headers this model adds to the provider's.
    pub headers: BTreeMap<String, String>,
    /// Request-body overrides for this model.
    pub compat: BTreeMap<String, JsonValue>,
    /// How each thinking level serializes for this model.
    pub thinking_level_map: BTreeMap<ThinkingLevel, Option<String>>,
}

impl ProviderModelMetadata {
    /// Serialize this model metadata to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        let mut object: Map<String, JsonValue> = Map::new();
        object.insert(
            "name".to_string(),
            json_optional_string(self.name.as_deref()),
        );
        object.insert(
            "api".to_string(),
            json_optional_string(self.api.map(ProviderApi::as_str)),
        );
        object.insert(
            "base_url".to_string(),
            json_optional_string(self.base_url.as_deref()),
        );
        object.insert(
            "reasoning".to_string(),
            self.reasoning.map_or(JsonValue::Null, JsonValue::from),
        );
        object.insert("input".to_string(), json_strings(&self.input));
        object.insert("cost".to_string(), json_number_map(&self.cost));
        object.insert(
            "cost_tiers".to_string(),
            JsonValue::Array(self.cost_tiers.iter().map(ModelCostTier::to_json).collect()),
        );
        object.insert(
            "context_window".to_string(),
            self.context_window.map_or(JsonValue::Null, JsonValue::from),
        );
        object.insert(
            "max_tokens".to_string(),
            self.max_tokens.map_or(JsonValue::Null, JsonValue::from),
        );
        object.insert("headers".to_string(), json_string_map(&self.headers));
        object.insert("compat".to_string(), json_value_map(&self.compat));
        object.insert(
            "thinking_level_map".to_string(),
            json_thinking_level_map(&self.thinking_level_map),
        );
        JsonValue::Object(object)
    }
}

/// Durable settings for one OpenAI-compatible provider.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiCompatibleProviderConfig {
    /// The provider id.
    pub name: String,
    /// The endpoint.
    pub base_url: String,
    /// The wire protocol.
    pub api: ProviderApi,
    /// The environment variable holding the API key.
    pub api_key_env: String,
    /// The stored credential name, when the provider has one.
    pub credential_name: Option<String>,
    /// The declared models.
    pub models: Vec<String>,
    /// The model used when none is selected.
    pub default_model: String,
    /// Per-model context windows.
    pub context_windows: BTreeMap<String, i64>,
    /// Headers sent with every request.
    pub headers: BTreeMap<String, String>,
    /// Request-body overrides for this provider.
    pub compat: BTreeMap<String, JsonValue>,
    /// Per-model metadata.
    pub model_metadata: BTreeMap<String, ProviderModelMetadata>,
    /// The request timeout in seconds.
    pub timeout_seconds: f64,
    /// The number of retries after a failed request.
    pub max_retries: i64,
    /// The cap on the exponential retry backoff, in seconds.
    pub max_retry_delay_seconds: f64,
    /// The thinking levels this provider supports.
    pub thinking_levels: Option<Vec<ThinkingLevel>>,
    /// The models that change reasoning effort independently.
    pub thinking_models: Vec<String>,
    /// The preferred thinking level.
    pub thinking_default: Option<ThinkingLevel>,
    /// The field a thinking level is serialized into.
    pub thinking_parameter: Option<ThinkingParameter>,
    /// Per-model preferred thinking levels.
    pub thinking_defaults: BTreeMap<String, ThinkingLevel>,
    /// The Hugging Face inference provider chosen per model.
    pub inference_providers: BTreeMap<String, String>,
}

impl OpenAiCompatibleProviderConfig {
    /// A provider named `name` with the reference's defaults.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_url: DEFAULT_OPENAI_COMPATIBLE_BASE_URL.to_string(),
            api: ProviderApi::OpenAiCompletions,
            api_key_env: DEFAULT_OPENAI_API_KEY_ENV.to_string(),
            credential_name: None,
            models: vec![DEFAULT_MODEL.to_string()],
            default_model: DEFAULT_MODEL.to_string(),
            context_windows: BTreeMap::new(),
            headers: BTreeMap::new(),
            compat: BTreeMap::new(),
            model_metadata: BTreeMap::new(),
            timeout_seconds: DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS,
            max_retries: DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES,
            max_retry_delay_seconds: DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS,
            thinking_levels: None,
            thinking_models: Vec::new(),
            thinking_default: None,
            thinking_parameter: None,
            thinking_defaults: BTreeMap::new(),
            inference_providers: BTreeMap::new(),
        }
    }

    /// Validate every configured value.
    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        validate_provider_numbers(
            self.timeout_seconds,
            self.max_retries,
            self.max_retry_delay_seconds,
        )?;
        validate_context_windows(&self.context_windows)?;
        validate_model_metadata(&self.models, &self.model_metadata)?;
        validate_json_object(&self.compat, "Provider compat")?;
        validate_thinking_config(
            self.thinking_levels.as_deref(),
            &self.thinking_models,
            self.thinking_default,
            self.thinking_parameter,
        )?;
        validate_thinking_defaults(&self.thinking_defaults)?;
        validate_inference_providers(&self.name, &self.models, &self.inference_providers)
    }

    /// Serialize this provider config to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        let mut object: Map<String, JsonValue> = Map::new();
        object.insert("name".to_string(), JsonValue::from(self.name.clone()));
        object.insert("type".to_string(), JsonValue::from(OPENAI_COMPATIBLE_TYPE));
        object.insert(
            "base_url".to_string(),
            JsonValue::from(self.base_url.clone()),
        );
        object.insert("api".to_string(), JsonValue::from(self.api.as_str()));
        object.insert(
            "api_key_env".to_string(),
            JsonValue::from(self.api_key_env.clone()),
        );
        insert_provider_common_json(&mut object, self.credential_name.as_deref());
        insert_model_json(
            &mut object,
            &self.models,
            &self.default_model,
            &self.context_windows,
            &self.headers,
            &self.model_metadata,
        );
        object.insert("compat".to_string(), json_value_map(&self.compat));
        insert_provider_numbers_json(
            &mut object,
            self.timeout_seconds,
            self.max_retries,
            self.max_retry_delay_seconds,
        );
        insert_thinking_json(
            &mut object,
            self.thinking_levels.as_deref(),
            &self.thinking_models,
            self.thinking_default,
            self.thinking_parameter,
            &self.thinking_defaults,
        );
        object.insert(
            "inference_providers".to_string(),
            json_string_map(&self.inference_providers),
        );
        JsonValue::Object(object)
    }
}

/// Durable settings for Anthropic's Messages API.
#[derive(Debug, Clone, PartialEq)]
pub struct AnthropicProviderConfig {
    /// The provider id.
    pub name: String,
    /// The endpoint.
    pub base_url: String,
    /// The wire protocol.
    pub api: ProviderApi,
    /// The environment variable holding the API key.
    pub api_key_env: String,
    /// The stored credential name, when the provider has one.
    pub credential_name: Option<String>,
    /// The declared models.
    pub models: Vec<String>,
    /// The model used when none is selected.
    pub default_model: String,
    /// Per-model context windows.
    pub context_windows: BTreeMap<String, i64>,
    /// Headers sent with every request.
    pub headers: BTreeMap<String, String>,
    /// Request-body overrides for this provider.
    pub compat: BTreeMap<String, JsonValue>,
    /// Per-model metadata.
    pub model_metadata: BTreeMap<String, ProviderModelMetadata>,
    /// The request timeout in seconds.
    pub timeout_seconds: f64,
    /// The number of retries after a failed request.
    pub max_retries: i64,
    /// The cap on the exponential retry backoff, in seconds.
    pub max_retry_delay_seconds: f64,
    /// The thinking levels this provider supports.
    pub thinking_levels: Option<Vec<ThinkingLevel>>,
    /// The models that change reasoning effort independently.
    pub thinking_models: Vec<String>,
    /// The preferred thinking level.
    pub thinking_default: Option<ThinkingLevel>,
    /// The field a thinking level is serialized into.
    pub thinking_parameter: Option<ThinkingParameter>,
    /// Per-model preferred thinking levels.
    pub thinking_defaults: BTreeMap<String, ThinkingLevel>,
}

impl AnthropicProviderConfig {
    /// A provider named `name` with the reference's defaults.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_url: DEFAULT_ANTHROPIC_BASE_URL.to_string(),
            api: ProviderApi::AnthropicMessages,
            api_key_env: DEFAULT_ANTHROPIC_API_KEY_ENV.to_string(),
            credential_name: Some(ANTHROPIC_TYPE.to_string()),
            models: vec![DEFAULT_ANTHROPIC_MODEL.to_string()],
            default_model: DEFAULT_ANTHROPIC_MODEL.to_string(),
            context_windows: BTreeMap::new(),
            headers: BTreeMap::new(),
            compat: BTreeMap::new(),
            model_metadata: BTreeMap::new(),
            timeout_seconds: DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS,
            max_retries: DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES,
            max_retry_delay_seconds: DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS,
            thinking_levels: None,
            thinking_models: Vec::new(),
            thinking_default: None,
            thinking_parameter: None,
            thinking_defaults: BTreeMap::new(),
        }
    }

    /// Validate every configured value.
    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        validate_provider_numbers(
            self.timeout_seconds,
            self.max_retries,
            self.max_retry_delay_seconds,
        )?;
        validate_context_windows(&self.context_windows)?;
        validate_model_metadata(&self.models, &self.model_metadata)?;
        validate_json_object(&self.compat, "Provider compat")?;
        validate_thinking_config(
            self.thinking_levels.as_deref(),
            &self.thinking_models,
            self.thinking_default,
            self.thinking_parameter,
        )?;
        validate_thinking_defaults(&self.thinking_defaults)
    }

    /// Serialize this provider config to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        let mut object: Map<String, JsonValue> = Map::new();
        object.insert("name".to_string(), JsonValue::from(self.name.clone()));
        object.insert("type".to_string(), JsonValue::from(ANTHROPIC_TYPE));
        object.insert(
            "base_url".to_string(),
            JsonValue::from(self.base_url.clone()),
        );
        object.insert("api".to_string(), JsonValue::from(self.api.as_str()));
        object.insert(
            "api_key_env".to_string(),
            JsonValue::from(self.api_key_env.clone()),
        );
        insert_provider_common_json(&mut object, self.credential_name.as_deref());
        insert_model_json(
            &mut object,
            &self.models,
            &self.default_model,
            &self.context_windows,
            &self.headers,
            &self.model_metadata,
        );
        object.insert("compat".to_string(), json_value_map(&self.compat));
        insert_provider_numbers_json(
            &mut object,
            self.timeout_seconds,
            self.max_retries,
            self.max_retry_delay_seconds,
        );
        insert_thinking_json(
            &mut object,
            self.thinking_levels.as_deref(),
            &self.thinking_models,
            self.thinking_default,
            self.thinking_parameter,
            &self.thinking_defaults,
        );
        JsonValue::Object(object)
    }
}

/// Durable settings for OpenAI Codex subscription OAuth.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiCodexProviderConfig {
    /// The provider id.
    pub name: String,
    /// The endpoint.
    pub base_url: String,
    /// The environment variable holding the access token.
    pub api_key_env: String,
    /// The stored credential name, when the provider has one.
    pub credential_name: Option<String>,
    /// The declared models.
    pub models: Vec<String>,
    /// The model used when none is selected.
    pub default_model: String,
    /// Per-model context windows.
    pub context_windows: BTreeMap<String, i64>,
    /// Headers sent with every request.
    pub headers: BTreeMap<String, String>,
    /// Per-model metadata.
    pub model_metadata: BTreeMap<String, ProviderModelMetadata>,
    /// The request timeout in seconds.
    pub timeout_seconds: f64,
    /// The number of retries after a failed request.
    pub max_retries: i64,
    /// The cap on the exponential retry backoff, in seconds.
    pub max_retry_delay_seconds: f64,
    /// The thinking levels this provider supports.
    pub thinking_levels: Option<Vec<ThinkingLevel>>,
    /// The models that change reasoning effort independently.
    pub thinking_models: Vec<String>,
    /// The preferred thinking level.
    pub thinking_default: Option<ThinkingLevel>,
    /// The field a thinking level is serialized into.
    pub thinking_parameter: Option<ThinkingParameter>,
    /// Per-model preferred thinking levels.
    pub thinking_defaults: BTreeMap<String, ThinkingLevel>,
}

impl OpenAiCodexProviderConfig {
    /// A provider named `name` with the reference's defaults.
    pub fn new(name: impl Into<String>) -> Self {
        let models: Vec<String> = OPENAI_CODEX_MODELS
            .iter()
            .map(|model| (*model).to_string())
            .collect();
        Self {
            name: name.into(),
            base_url: DEFAULT_OPENAI_CODEX_BASE_URL.to_string(),
            api_key_env: DEFAULT_OPENAI_CODEX_API_KEY_ENV.to_string(),
            credential_name: Some(OPENAI_CODEX_TYPE.to_string()),
            default_model: models
                .first()
                .cloned()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            models,
            context_windows: BTreeMap::new(),
            headers: BTreeMap::new(),
            model_metadata: BTreeMap::new(),
            timeout_seconds: DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS,
            max_retries: DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES,
            max_retry_delay_seconds: DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS,
            thinking_levels: None,
            thinking_models: Vec::new(),
            thinking_default: None,
            thinking_parameter: None,
            thinking_defaults: BTreeMap::new(),
        }
    }

    /// Validate every configured value.
    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        validate_provider_numbers(
            self.timeout_seconds,
            self.max_retries,
            self.max_retry_delay_seconds,
        )?;
        validate_context_windows(&self.context_windows)?;
        validate_model_metadata(&self.models, &self.model_metadata)?;
        validate_thinking_config(
            self.thinking_levels.as_deref(),
            &self.thinking_models,
            self.thinking_default,
            self.thinking_parameter,
        )?;
        validate_thinking_defaults(&self.thinking_defaults)
    }

    /// Serialize this provider config to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        let mut object: Map<String, JsonValue> = Map::new();
        object.insert("name".to_string(), JsonValue::from(self.name.clone()));
        object.insert("type".to_string(), JsonValue::from(OPENAI_CODEX_TYPE));
        object.insert(
            "base_url".to_string(),
            JsonValue::from(self.base_url.clone()),
        );
        object.insert(
            "api_key_env".to_string(),
            JsonValue::from(self.api_key_env.clone()),
        );
        insert_provider_common_json(&mut object, self.credential_name.as_deref());
        insert_model_json(
            &mut object,
            &self.models,
            &self.default_model,
            &self.context_windows,
            &self.headers,
            &self.model_metadata,
        );
        insert_provider_numbers_json(
            &mut object,
            self.timeout_seconds,
            self.max_retries,
            self.max_retry_delay_seconds,
        );
        insert_thinking_json(
            &mut object,
            self.thinking_levels.as_deref(),
            &self.thinking_models,
            self.thinking_default,
            self.thinking_parameter,
            &self.thinking_defaults,
        );
        JsonValue::Object(object)
    }
}

/// A durable provider configuration.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderConfig {
    /// An OpenAI-compatible endpoint.
    OpenAiCompatible(OpenAiCompatibleProviderConfig),
    /// Anthropic's Messages API.
    Anthropic(AnthropicProviderConfig),
    /// The OpenAI Codex subscription.
    OpenAiCodex(OpenAiCodexProviderConfig),
}

impl ProviderConfig {
    /// The provider id.
    pub fn name(&self) -> &str {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.name,
            ProviderConfig::Anthropic(config) => &config.name,
            ProviderConfig::OpenAiCodex(config) => &config.name,
        }
    }

    /// The durable kind of this provider.
    pub fn kind(&self) -> ProviderKind {
        provider_kind(self)
    }

    /// The endpoint.
    pub fn base_url(&self) -> &str {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.base_url,
            ProviderConfig::Anthropic(config) => &config.base_url,
            ProviderConfig::OpenAiCodex(config) => &config.base_url,
        }
    }

    /// The wire protocol, where the provider kind has one.
    pub fn api(&self) -> Option<ProviderApi> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => Some(config.api),
            ProviderConfig::Anthropic(config) => Some(config.api),
            ProviderConfig::OpenAiCodex(_) => None,
        }
    }

    /// The environment variable holding the API key.
    pub fn api_key_env(&self) -> &str {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.api_key_env,
            ProviderConfig::Anthropic(config) => &config.api_key_env,
            ProviderConfig::OpenAiCodex(config) => &config.api_key_env,
        }
    }

    /// The stored credential name, when the provider has one.
    pub fn credential_name(&self) -> Option<&str> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.credential_name.as_deref(),
            ProviderConfig::Anthropic(config) => config.credential_name.as_deref(),
            ProviderConfig::OpenAiCodex(config) => config.credential_name.as_deref(),
        }
    }

    /// The declared models.
    pub fn models(&self) -> &[String] {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.models,
            ProviderConfig::Anthropic(config) => &config.models,
            ProviderConfig::OpenAiCodex(config) => &config.models,
        }
    }

    /// The model used when none is selected.
    pub fn default_model(&self) -> &str {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.default_model,
            ProviderConfig::Anthropic(config) => &config.default_model,
            ProviderConfig::OpenAiCodex(config) => &config.default_model,
        }
    }

    /// Per-model context windows.
    pub fn context_windows(&self) -> &BTreeMap<String, i64> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.context_windows,
            ProviderConfig::Anthropic(config) => &config.context_windows,
            ProviderConfig::OpenAiCodex(config) => &config.context_windows,
        }
    }

    /// Headers sent with every request.
    pub fn headers(&self) -> &BTreeMap<String, String> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.headers,
            ProviderConfig::Anthropic(config) => &config.headers,
            ProviderConfig::OpenAiCodex(config) => &config.headers,
        }
    }

    /// The request timeout in seconds.
    pub fn timeout_seconds(&self) -> f64 {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.timeout_seconds,
            ProviderConfig::Anthropic(config) => config.timeout_seconds,
            ProviderConfig::OpenAiCodex(config) => config.timeout_seconds,
        }
    }

    /// The number of retries after a failed request.
    pub fn max_retries(&self) -> i64 {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.max_retries,
            ProviderConfig::Anthropic(config) => config.max_retries,
            ProviderConfig::OpenAiCodex(config) => config.max_retries,
        }
    }

    /// The cap on the exponential retry backoff, in seconds.
    pub fn max_retry_delay_seconds(&self) -> f64 {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.max_retry_delay_seconds,
            ProviderConfig::Anthropic(config) => config.max_retry_delay_seconds,
            ProviderConfig::OpenAiCodex(config) => config.max_retry_delay_seconds,
        }
    }

    /// Per-model metadata.
    pub fn model_metadata(&self) -> &BTreeMap<String, ProviderModelMetadata> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.model_metadata,
            ProviderConfig::Anthropic(config) => &config.model_metadata,
            ProviderConfig::OpenAiCodex(config) => &config.model_metadata,
        }
    }

    /// The thinking levels this provider supports.
    pub fn thinking_levels(&self) -> Option<&[ThinkingLevel]> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.thinking_levels.as_deref(),
            ProviderConfig::Anthropic(config) => config.thinking_levels.as_deref(),
            ProviderConfig::OpenAiCodex(config) => config.thinking_levels.as_deref(),
        }
    }

    /// The models that change reasoning effort independently.
    pub fn thinking_models(&self) -> &[String] {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.thinking_models,
            ProviderConfig::Anthropic(config) => &config.thinking_models,
            ProviderConfig::OpenAiCodex(config) => &config.thinking_models,
        }
    }

    /// The preferred thinking level.
    pub fn thinking_default(&self) -> Option<ThinkingLevel> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.thinking_default,
            ProviderConfig::Anthropic(config) => config.thinking_default,
            ProviderConfig::OpenAiCodex(config) => config.thinking_default,
        }
    }

    /// The field a thinking level is serialized into.
    pub fn thinking_parameter(&self) -> Option<ThinkingParameter> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.thinking_parameter,
            ProviderConfig::Anthropic(config) => config.thinking_parameter,
            ProviderConfig::OpenAiCodex(config) => config.thinking_parameter,
        }
    }

    /// Per-model preferred thinking levels.
    pub fn thinking_defaults(&self) -> &BTreeMap<String, ThinkingLevel> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => &config.thinking_defaults,
            ProviderConfig::Anthropic(config) => &config.thinking_defaults,
            ProviderConfig::OpenAiCodex(config) => &config.thinking_defaults,
        }
    }

    /// Validate every configured value.
    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.validate(),
            ProviderConfig::Anthropic(config) => config.validate(),
            ProviderConfig::OpenAiCodex(config) => config.validate(),
        }
    }

    /// Serialize this provider config to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        match self {
            ProviderConfig::OpenAiCompatible(config) => config.to_json(),
            ProviderConfig::Anthropic(config) => config.to_json(),
            ProviderConfig::OpenAiCodex(config) => config.to_json(),
        }
    }
}

/// Return the durable provider kind.
pub fn provider_kind(provider: &ProviderConfig) -> ProviderKind {
    match provider {
        ProviderConfig::Anthropic(_) => ProviderKind::Anthropic,
        ProviderConfig::OpenAiCodex(_) => ProviderKind::OpenAiCodex,
        ProviderConfig::OpenAiCompatible(config) => match config.api {
            ProviderApi::GoogleGenerativeAi => ProviderKind::GoogleGenerativeAi,
            ProviderApi::MistralConversations => ProviderKind::MistralConversations,
            _ => ProviderKind::OpenAiCompatible,
        },
    }
}

/// A provider/model pair enabled for quick model cycling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedModelConfig {
    /// The provider id.
    pub provider: String,
    /// The model id.
    pub model: String,
}

impl ScopedModelConfig {
    /// A provider/model pair.
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// Serialize this scoped model reference.
    pub fn to_json(&self) -> JsonValue {
        let mut object: Map<String, JsonValue> = Map::new();
        object.insert(
            "provider".to_string(),
            JsonValue::from(self.provider.clone()),
        );
        object.insert("model".to_string(), JsonValue::from(self.model.clone()));
        JsonValue::Object(object)
    }
}

/// Tau provider settings.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProviderSettings {
    /// The provider selected when none is named.
    pub default_provider: String,
    /// The configured providers.
    pub providers: Vec<ProviderConfig>,
    /// The provider/model pairs enabled for quick cycling.
    pub scoped_models: Vec<ScopedModelConfig>,
}

impl ProviderSettings {
    /// Settings whose providers are exactly `catalog`.
    pub fn from_catalog(catalog: &[ProviderConfig]) -> Self {
        Self {
            default_provider: DEFAULT_PROVIDER_NAME.to_string(),
            providers: catalog.to_vec(),
            scoped_models: Vec::new(),
        }
    }

    /// Return a configured provider by name, defaulting to the selection.
    pub fn get_provider(&self, name: Option<&str>) -> Result<&ProviderConfig, ProviderConfigError> {
        let target = name.unwrap_or(&self.default_provider);
        self.providers
            .iter()
            .find(|provider| provider.name() == target)
            .ok_or_else(|| ProviderConfigError::new(format!("Unknown provider: {target}")))
    }

    /// Serialize runtime preferences to JSON-compatible data.
    pub fn to_json(&self) -> JsonValue {
        let mut preferences: Map<String, JsonValue> = Map::new();
        for provider in &self.providers {
            preferences.insert(
                provider.name().to_string(),
                provider_preference_to_json(provider),
            );
        }

        let mut object: Map<String, JsonValue> = Map::new();
        object.insert(
            "schema_version".to_string(),
            JsonValue::from(PROVIDER_SETTINGS_SCHEMA_VERSION),
        );
        object.insert(
            "default_provider".to_string(),
            JsonValue::from(self.default_provider.clone()),
        );
        object.insert(
            "provider_preferences".to_string(),
            JsonValue::Object(preferences),
        );
        object.insert(
            "scoped_models".to_string(),
            JsonValue::Array(
                self.scoped_models
                    .iter()
                    .map(ScopedModelConfig::to_json)
                    .collect(),
            ),
        );
        JsonValue::Object(object)
    }
}

/// Resolved provider/model selection for a run.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSelection {
    /// The selected provider.
    pub provider: ProviderConfig,
    /// The selected model.
    pub model: String,
}

/// Return the durable provider settings path.
pub fn provider_settings_path(paths: &TauPaths) -> PathBuf {
    paths.home().join(PROVIDER_SETTINGS_FILE_NAME)
}

/// Load durable provider settings, falling back to the catalog when the file is
/// absent.
pub fn load_provider_settings(
    paths: &TauPaths,
    catalog: &[ProviderConfig],
) -> Result<ProviderSettings, ProviderConfigError> {
    let path = provider_settings_path(paths);
    if !path.exists() {
        return Ok(ProviderSettings::from_catalog(catalog));
    }

    let text = std::fs::read_to_string(&path)
        .map_err(|error| ProviderConfigError::new(error.to_string()))?;
    let raw: JsonValue =
        serde_json::from_str(&text).map_err(|error| ProviderConfigError::new(error.to_string()))?;
    if !raw.is_object() {
        return Err(ProviderConfigError::new(
            "Provider settings must be a JSON object",
        ));
    }

    provider_settings_from_json(&raw, catalog)
}

/// Parse provider preferences from JSON-compatible data.
///
/// The current shape stores runtime preferences under `provider_preferences`.
/// The older `providers[]` shape is still accepted for compatibility; rewriting
/// it to the current shape is a write, which this module does not port.
///
/// A preference that names a provider the catalog does not define is ignored:
/// preferences carry runtime overrides, never provider definitions, so a stale
/// entry must not stop Tau from starting or from registering the provider again.
pub fn provider_settings_from_json(
    data: &JsonValue,
    catalog: &[ProviderConfig],
) -> Result<ProviderSettings, ProviderConfigError> {
    let JsonValue::Object(object) = data else {
        return Err(ProviderConfigError::new(
            "Provider settings must be a JSON object",
        ));
    };

    match object.get("schema_version") {
        None | Some(JsonValue::Null) => {}
        Some(value)
            if value.as_i64() == Some(PROVIDER_SETTINGS_SCHEMA_VERSION)
                || value.as_f64() == Some(PROVIDER_SETTINGS_SCHEMA_VERSION as f64) => {}
        Some(value) => {
            return Err(ProviderConfigError::new(format!(
                "Unsupported provider settings schema_version: {}",
                python_repr(value)
            )));
        }
    }

    let default_provider = string_field(object.get("default_provider"), "default_provider")?;
    let scoped_models = scoped_models_from_json(object.get("scoped_models"))?;

    if object.contains_key("provider_preferences") {
        let providers = providers_with_preferences(object.get("provider_preferences"), catalog)?;
        return Ok(ProviderSettings {
            default_provider,
            providers,
            scoped_models,
        });
    }

    let providers_data = object.get("providers");
    let Some(JsonValue::Array(entries)) = providers_data else {
        return Err(ProviderConfigError::new(
            "Provider settings must include provider_preferences or legacy providers",
        ));
    };
    if entries.is_empty() {
        return Err(ProviderConfigError::new(
            "Provider settings must include provider_preferences or legacy providers",
        ));
    }

    let mut providers = Vec::with_capacity(entries.len());
    for entry in entries {
        providers.push(provider_config_from_json(entry)?);
    }

    let mut names = BTreeSet::new();
    for provider in &providers {
        if !names.insert(provider.name().to_string()) {
            return Err(ProviderConfigError::new("Provider names must be unique"));
        }
    }

    Ok(ProviderSettings {
        default_provider,
        providers,
        scoped_models,
    })
}

/// Create a durable provider config from one legacy `providers[]` record.
pub fn provider_config_from_json(data: &JsonValue) -> Result<ProviderConfig, ProviderConfigError> {
    let JsonValue::Object(object) = data else {
        return Err(ProviderConfigError::new(
            "Provider entries must be JSON objects",
        ));
    };

    let provider_type = string_field(object.get("type"), "providers[].type")?;
    let Some(kind) = ProviderKind::parse(&provider_type) else {
        return Err(ProviderConfigError::new(format!(
            "Unsupported provider type: {provider_type}"
        )));
    };

    let name = string_field(object.get("name"), "providers[].name")?;
    let field = |field: &str| format!("providers[{name}].{field}");
    let base_url = string_field(object.get("base_url"), &field("base_url"))?
        .trim_end_matches('/')
        .to_string();
    let api = optional_provider_api_field(object.get("api"), &field("api"))?;
    let api_key_env = string_field(object.get("api_key_env"), &field("api_key_env"))?;
    let credential_name =
        optional_string_field(object.get("credential_name"), &field("credential_name"))?;
    let mut models = string_tuple_field(object.get("models"), &field("models"))?;
    let default_model = string_field(object.get("default_model"), &field("default_model"))?;
    let context_windows = context_window_dict_field(
        object.get("context_windows").unwrap_or(&empty_object()),
        &field("context_windows"),
    )?;
    let headers = string_dict_field(
        object.get("headers").unwrap_or(&empty_object()),
        &field("headers"),
    )?;
    let compat = json_dict_field(
        object.get("compat").unwrap_or(&empty_object()),
        &field("compat"),
    )?;
    let model_metadata = model_metadata_dict_field(
        object.get("model_metadata").unwrap_or(&empty_object()),
        &models,
        &field("model_metadata"),
    )?;
    let timeout_seconds = positive_float_field(
        object
            .get("timeout_seconds")
            .unwrap_or(&JsonValue::from(DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS)),
        &field("timeout_seconds"),
    )?;
    let max_retries = non_negative_int_field(
        object
            .get("max_retries")
            .unwrap_or(&JsonValue::from(DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES)),
        &field("max_retries"),
    )?;
    let max_retry_delay_seconds = non_negative_float_field(
        object
            .get("max_retry_delay_seconds")
            .unwrap_or(&JsonValue::from(
                DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS,
            )),
        &field("max_retry_delay_seconds"),
    )?;
    let thinking_levels =
        optional_thinking_levels_field(object.get("thinking_levels"), &field("thinking_levels"))?;
    let thinking_models =
        optional_string_tuple_field(object.get("thinking_models"), &field("thinking_models"))?;
    let thinking_default =
        optional_thinking_level_field(object.get("thinking_default"), &field("thinking_default"))?;
    let thinking_parameter = optional_thinking_parameter_field(
        object.get("thinking_parameter"),
        &field("thinking_parameter"),
    )?;
    let thinking_defaults = raw_thinking_defaults_field(
        object.get("thinking_defaults").unwrap_or(&empty_object()),
        &field("thinking_defaults"),
    )?;

    if !models.iter().any(|model| model == &default_model) {
        models.push(default_model.clone());
    }

    match kind {
        ProviderKind::Anthropic => {
            let config = AnthropicProviderConfig {
                name,
                base_url,
                api: api.unwrap_or_else(|| kind.default_api()),
                api_key_env,
                credential_name,
                models,
                default_model,
                context_windows,
                headers,
                compat,
                model_metadata,
                timeout_seconds,
                max_retries,
                max_retry_delay_seconds,
                thinking_levels,
                thinking_models,
                thinking_default,
                thinking_parameter,
                thinking_defaults,
            };
            config.validate()?;
            Ok(ProviderConfig::Anthropic(config))
        }
        ProviderKind::OpenAiCodex => {
            reject_codex_legacy_compat(&compat)?;
            let config = OpenAiCodexProviderConfig {
                name,
                base_url,
                api_key_env,
                credential_name,
                models,
                default_model,
                context_windows,
                headers,
                model_metadata,
                timeout_seconds,
                max_retries,
                max_retry_delay_seconds,
                thinking_levels,
                thinking_models,
                thinking_default,
                thinking_parameter,
                thinking_defaults,
            };
            config.validate()?;
            Ok(ProviderConfig::OpenAiCodex(config))
        }
        _ => {
            let config = OpenAiCompatibleProviderConfig {
                name,
                base_url,
                api: api.unwrap_or_else(|| kind.default_api()),
                api_key_env,
                credential_name,
                models,
                default_model,
                context_windows,
                headers,
                compat,
                model_metadata,
                timeout_seconds,
                max_retries,
                max_retry_delay_seconds,
                thinking_levels,
                thinking_models,
                thinking_default,
                thinking_parameter,
                thinking_defaults,
                inference_providers: BTreeMap::new(),
            };
            config.validate()?;
            Ok(ProviderConfig::OpenAiCompatible(config))
        }
    }
}

/// Resolve the provider and model for a run.
pub fn resolve_provider_selection(
    settings: &ProviderSettings,
    provider_name: Option<&str>,
    model: Option<&str>,
) -> Result<ProviderSelection, ProviderConfigError> {
    let provider = settings.get_provider(provider_name)?;
    let selected_model = match model {
        Some(model) if !model.is_empty() => model.to_string(),
        _ => provider.default_model().to_string(),
    };
    if selected_model.is_empty() {
        return Err(ProviderConfigError::new(format!(
            "Provider {} does not define a default model",
            provider.name()
        )));
    }
    validate_provider_model(provider, &selected_model)?;

    Ok(ProviderSelection {
        provider: provider.clone(),
        model: selected_model,
    })
}

/// Reject a model that `provider` does not declare.
pub fn validate_provider_model(
    provider: &ProviderConfig,
    model: &str,
) -> Result<(), ProviderConfigError> {
    if provider.models().iter().any(|declared| declared == model) {
        return Ok(());
    }

    let mut available: Vec<&str> = provider.models().iter().map(String::as_str).collect();
    available.sort_unstable();
    let available = if available.is_empty() {
        "none".to_string()
    } else {
        available.join(", ")
    };
    Err(ProviderConfigError::new(format!(
        "Model is not configured for provider {}: {model}. Available models: {available}",
        provider.name()
    )))
}

/// Return the thinking levels a provider/model pair supports.
pub fn provider_thinking_levels(
    provider: &ProviderConfig,
    model: Option<&str>,
) -> Vec<ThinkingLevel> {
    let selected_model = model.unwrap_or_else(|| provider.default_model());
    let metadata = provider.model_metadata().get(selected_model);

    if metadata.is_some_and(|metadata| metadata.reasoning == Some(false)) {
        return Vec::new();
    }

    let Some(levels) = provider.thinking_levels() else {
        let Some(metadata) = metadata else {
            return Vec::new();
        };
        if metadata.reasoning != Some(true) {
            return Vec::new();
        }
        return levels_from_thinking_map(&metadata.thinking_level_map);
    };

    if !provider.thinking_models().is_empty()
        && !provider
            .thinking_models()
            .iter()
            .any(|declared| declared == selected_model)
    {
        return Vec::new();
    }

    ThinkingLevel::ALL
        .into_iter()
        .filter(|level| {
            let declared = levels.contains(level)
                || metadata.is_some_and(|metadata| {
                    metadata
                        .thinking_level_map
                        .get(level)
                        .is_some_and(Option::is_some)
                });
            declared
                && metadata
                    .is_none_or(|metadata| metadata_supports_thinking_level(metadata, *level))
        })
        .collect()
}

/// Return the preferred thinking level for a provider/model pair.
pub fn provider_default_thinking_level(
    provider: &ProviderConfig,
    model: Option<&str>,
) -> Option<ThinkingLevel> {
    let levels = provider_thinking_levels(provider, model);
    if levels.is_empty() {
        return None;
    }
    if let Some(default) = provider.thinking_default()
        && levels.contains(&default)
    {
        return Some(default);
    }
    if levels.contains(&ThinkingLevel::DEFAULT) {
        return Some(ThinkingLevel::DEFAULT);
    }
    levels.first().copied()
}

/// Return the output token limit for a model, or `None` when it is unset.
pub fn provider_model_max_tokens(provider: &ProviderConfig, model: Option<&str>) -> Option<i64> {
    let selected_model = model.unwrap_or_else(|| provider.default_model());
    provider
        .model_metadata()
        .get(selected_model)
        .and_then(|metadata| metadata.max_tokens)
}

/// Return whether a model accepts images.
pub fn provider_model_supports_images(provider: &ProviderConfig, model: Option<&str>) -> bool {
    let selected_model = model.unwrap_or_else(|| provider.default_model());
    provider
        .model_metadata()
        .get(selected_model)
        .is_some_and(|metadata| metadata.input.iter().any(|input| input == "image"))
}

/// Return the endpoint a model is served from.
pub fn provider_base_url(provider: &ProviderConfig, model: Option<&str>) -> String {
    let selected_model = model.unwrap_or_else(|| provider.default_model());
    provider
        .model_metadata()
        .get(selected_model)
        .and_then(|metadata| metadata.base_url.as_deref())
        .filter(|base_url| !base_url.is_empty())
        .unwrap_or(provider.base_url())
        .to_string()
}

/// Return the headers a model is sent with, model metadata taking precedence.
pub fn provider_headers(
    provider: &ProviderConfig,
    model: Option<&str>,
) -> BTreeMap<String, String> {
    let selected_model = model.unwrap_or_else(|| provider.default_model());
    let mut headers = provider.headers().clone();
    if let Some(metadata) = provider.model_metadata().get(selected_model) {
        headers.extend(metadata.headers.clone());
    }
    headers
}

/// Resolve the OpenAI-compatible endpoint, including the `OPENAI_BASE_URL`
/// override and the trailing-slash trim the reference applies.
pub fn openai_compatible_base_url(
    provider: &ProviderConfig,
    env: &dyn EnvSource,
    model: Option<&str>,
) -> String {
    let mut base_url = provider_base_url(provider, model);
    if matches!(provider, ProviderConfig::OpenAiCompatible(_))
        && provider.name() == DEFAULT_PROVIDER_NAME
        && provider.api_key_env() == DEFAULT_OPENAI_API_KEY_ENV
        && let Some(override_url) = env.get(OPENAI_BASE_URL_VAR)
    {
        base_url = override_url;
    }
    base_url.trim_end_matches('/').to_string()
}

/// Resolve the API key for a provider.
///
/// A stored credential wins over the environment: the reference checks the
/// credential store first and only then `api_key_env`.
pub fn api_key_from_provider(
    provider: &ProviderConfig,
    env: &dyn EnvSource,
    reader: Option<&dyn CredentialReader>,
) -> Result<String, ProviderConfigError> {
    if let (Some(credential_name), Some(reader)) = (provider.credential_name(), reader) {
        if let Some(credential) = reader.get(credential_name)
            && !credential.is_empty()
        {
            return Ok(credential);
        }
        if let Some(access) = reader.get_oauth(credential_name)
            && !access.is_empty()
        {
            return Ok(access);
        }
    }

    if let Some(api_key) = env.get(provider.api_key_env())
        && !api_key.is_empty()
    {
        return Ok(api_key);
    }

    let credential_hint = if provider.credential_name().is_some() {
        format!(" or run /login {}", provider.name())
    } else {
        String::new()
    };
    Err(ProviderConfigError::new(format!(
        "Missing provider API key. Set {}{credential_hint}.",
        provider.api_key_env()
    )))
}

/// Return whether Tau can attempt calls for this provider without setup.
pub fn provider_has_usable_credentials(
    provider: &ProviderConfig,
    env: &dyn EnvSource,
    reader: Option<&dyn CredentialReader>,
) -> bool {
    if let (Some(credential_name), Some(reader)) = (provider.credential_name(), reader) {
        if reader.get_oauth(credential_name).is_some() {
            return true;
        }
        if reader
            .get(credential_name)
            .is_some_and(|credential| !credential.is_empty())
        {
            return true;
        }
    }

    env.get(provider.api_key_env())
        .is_some_and(|api_key| !api_key.is_empty())
}

/// Return a normalized explicit Hugging Face inference provider suffix.
pub fn validate_huggingface_inference_provider(value: &str) -> Result<String, ProviderConfigError> {
    let normalized = value.trim();
    let shape_ok = !normalized.is_empty()
        && normalized
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && normalized.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        });
    if !shape_ok {
        return Err(ProviderConfigError::new(
            "Hugging Face inference provider must contain only letters, numbers, '.', '_', or '-'",
        ));
    }
    if matches!(normalized, "fastest" | "cheapest" | "preferred") {
        return Err(ProviderConfigError::new(format!(
            "Hugging Face inference provider must be explicit, not routing policy: {normalized}"
        )));
    }
    Ok(normalized.to_string())
}

fn providers_with_preferences(
    value: Option<&JsonValue>,
    catalog: &[ProviderConfig],
) -> Result<Vec<ProviderConfig>, ProviderConfigError> {
    let Some(JsonValue::Object(preferences)) = value else {
        return Err(ProviderConfigError::new(
            "Provider settings field must be an object: provider_preferences",
        ));
    };

    let mut providers = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (name, preference) in preferences {
        if name.trim().is_empty() {
            return Err(ProviderConfigError::new(
                "Provider preference names must be non-empty strings",
            ));
        }
        let provider_name = name.trim();
        if seen.contains(provider_name) {
            return Err(ProviderConfigError::new(
                "Provider preference names must be unique",
            ));
        }

        let Some(base) = catalog
            .iter()
            .find(|provider| provider.name() == provider_name)
        else {
            continue;
        };
        providers.push(apply_provider_preference(base, preference)?);
        seen.insert(provider_name);
    }
    Ok(providers)
}

fn apply_provider_preference(
    provider: &ProviderConfig,
    value: &JsonValue,
) -> Result<ProviderConfig, ProviderConfigError> {
    let JsonValue::Object(preference) = value else {
        return Err(ProviderConfigError::new(
            "Provider preference entries must be objects",
        ));
    };

    let name = provider.name();
    let field = |field: &str| format!("provider_preferences.{name}.{field}");

    // Provider preferences are user-level state shared across Tau versions, so
    // an option a newer version introduced is ignored rather than rejected.
    let default_model = match preference.get("default_model") {
        Some(raw) => {
            let candidate = string_field(Some(raw), &field("default_model"))?;
            if provider.models().iter().any(|model| model == &candidate) {
                candidate
            } else {
                provider.default_model().to_string()
            }
        }
        None => provider.default_model().to_string(),
    };
    let headers = match preference.get("headers") {
        Some(raw) => string_dict_field(raw, &field("headers"))?,
        None => provider.headers().clone(),
    };
    let timeout_seconds = match preference.get("timeout_seconds") {
        Some(raw) => positive_float_field(raw, &field("timeout_seconds"))?,
        None => provider.timeout_seconds(),
    };
    let max_retries = match preference.get("max_retries") {
        Some(raw) => non_negative_int_field(raw, &field("max_retries"))?,
        None => provider.max_retries(),
    };
    let max_retry_delay_seconds = match preference.get("max_retry_delay_seconds") {
        Some(raw) => non_negative_float_field(raw, &field("max_retry_delay_seconds"))?,
        None => provider.max_retry_delay_seconds(),
    };
    let thinking_defaults = match preference.get("thinking_defaults") {
        Some(raw) => {
            thinking_defaults_field(raw, provider, &field("thinking_defaults"), true, true)?
        }
        None => provider.thinking_defaults().clone(),
    };

    match provider {
        ProviderConfig::OpenAiCompatible(config) => {
            let inference_providers = match preference.get("inference_providers") {
                Some(raw) => {
                    let routes = string_dict_field(raw, &field("inference_providers"))?;
                    let routes: BTreeMap<String, String> = routes
                        .into_iter()
                        .filter(|(model, _)| config.models.iter().any(|known| known == model))
                        .collect();
                    validate_inference_providers(&config.name, &config.models, &routes)?;
                    routes
                }
                None => config.inference_providers.clone(),
            };

            Ok(ProviderConfig::OpenAiCompatible(
                OpenAiCompatibleProviderConfig {
                    default_model,
                    headers,
                    timeout_seconds,
                    max_retries,
                    max_retry_delay_seconds,
                    thinking_defaults,
                    inference_providers,
                    ..config.clone()
                },
            ))
        }
        ProviderConfig::Anthropic(config) => {
            Ok(ProviderConfig::Anthropic(AnthropicProviderConfig {
                default_model,
                headers,
                timeout_seconds,
                max_retries,
                max_retry_delay_seconds,
                thinking_defaults,
                ..config.clone()
            }))
        }
        ProviderConfig::OpenAiCodex(config) => {
            Ok(ProviderConfig::OpenAiCodex(OpenAiCodexProviderConfig {
                default_model,
                headers,
                timeout_seconds,
                max_retries,
                max_retry_delay_seconds,
                thinking_defaults,
                ..config.clone()
            }))
        }
    }
}

fn scoped_models_from_json(
    value: Option<&JsonValue>,
) -> Result<Vec<ScopedModelConfig>, ProviderConfigError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let JsonValue::Array(entries) = value else {
        return Err(ProviderConfigError::new(
            "Provider settings field must be a list: scoped_models",
        ));
    };

    let mut scoped: Vec<ScopedModelConfig> = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for entry in entries {
        let JsonValue::Object(entry) = entry else {
            return Err(ProviderConfigError::new(
                "Provider scoped_models entries must be objects",
            ));
        };
        let provider = string_field(entry.get("provider"), "scoped_models.provider")?;
        let model = string_field(entry.get("model"), "scoped_models.model")?;
        if seen.insert((provider.clone(), model.clone())) {
            scoped.push(ScopedModelConfig { provider, model });
        }
    }
    Ok(scoped)
}

fn thinking_defaults_field(
    value: &JsonValue,
    provider: &ProviderConfig,
    field_name: &str,
    ignore_unknown_models: bool,
    ignore_unavailable: bool,
) -> Result<BTreeMap<String, ThinkingLevel>, ProviderConfigError> {
    let mut raw = raw_thinking_defaults_field(value, field_name)?;
    if ignore_unknown_models {
        raw.retain(|model, _| provider.models().iter().any(|known| known == model));
    }

    let mut valid: BTreeMap<String, ThinkingLevel> = BTreeMap::new();
    for (model, thinking_level) in raw {
        validate_provider_model(provider, &model)?;
        let available = provider_thinking_levels(provider, Some(&model));
        if !available.contains(&thinking_level) {
            if ignore_unavailable {
                continue;
            }
            let modes = if available.is_empty() {
                "none".to_string()
            } else {
                available
                    .iter()
                    .map(|level| level.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(ProviderConfigError::new(format!(
                "Provider thinking default {} is not available for {}:{model}. \
                 Available modes: {modes}",
                thinking_level.as_str(),
                provider.name()
            )));
        }
        valid.insert(model, thinking_level);
    }
    Ok(valid)
}

fn raw_thinking_defaults_field(
    value: &JsonValue,
    field_name: &str,
) -> Result<BTreeMap<String, ThinkingLevel>, ProviderConfigError> {
    let JsonValue::Object(defaults) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be a thinking mode object: {field_name}"
        )));
    };

    let mut parsed: BTreeMap<String, ThinkingLevel> = BTreeMap::new();
    for (key, item) in defaults {
        let model = string_field(Some(&JsonValue::String(key.clone())), field_name)?;
        let thinking_level =
            optional_thinking_level_field(Some(item), &format!("{field_name}.{model}"))?;
        let Some(thinking_level) = thinking_level else {
            return Err(ProviderConfigError::new(format!(
                "Provider field must be a thinking mode: {field_name}"
            )));
        };
        parsed.insert(model, thinking_level);
    }
    Ok(parsed)
}

fn validate_provider_numbers(
    timeout_seconds: f64,
    max_retries: i64,
    max_retry_delay_seconds: f64,
) -> Result<(), ProviderConfigError> {
    if timeout_seconds <= 0.0 {
        return Err(ProviderConfigError::new(
            "Provider timeout_seconds must be greater than 0",
        ));
    }
    if max_retries < 0 {
        return Err(ProviderConfigError::new(
            "Provider max_retries must be 0 or greater",
        ));
    }
    if max_retry_delay_seconds < 0.0 {
        return Err(ProviderConfigError::new(
            "Provider max_retry_delay_seconds must be 0 or greater",
        ));
    }
    Ok(())
}

fn validate_context_windows(
    context_windows: &BTreeMap<String, i64>,
) -> Result<(), ProviderConfigError> {
    for (model, context_window) in context_windows {
        if model.trim().is_empty() {
            return Err(ProviderConfigError::new(
                "Provider context_windows keys must be non-empty strings",
            ));
        }
        if *context_window <= 0 {
            return Err(ProviderConfigError::new(
                "Provider context_windows values must be positive integers",
            ));
        }
    }
    Ok(())
}

fn validate_model_metadata(
    models: &[String],
    model_metadata: &BTreeMap<String, ProviderModelMetadata>,
) -> Result<(), ProviderConfigError> {
    for (model, metadata) in model_metadata {
        if !models.iter().any(|declared| declared == model) {
            return Err(ProviderConfigError::new(format!(
                "Provider model_metadata key is not in models: {model}"
            )));
        }
        if metadata.context_window.is_some_and(|window| window <= 0) {
            return Err(ProviderConfigError::new(
                "Provider model_metadata context_window must be positive",
            ));
        }
        if metadata
            .max_tokens
            .is_some_and(|max_tokens| max_tokens <= 0)
        {
            return Err(ProviderConfigError::new(
                "Provider model_metadata max_tokens must be positive",
            ));
        }
        if metadata
            .input
            .iter()
            .any(|input| !matches!(input.as_str(), "text" | "image"))
        {
            return Err(ProviderConfigError::new(
                "Provider model_metadata input must contain text or image",
            ));
        }
        if metadata.cost.values().any(|value| *value < 0.0) {
            return Err(ProviderConfigError::new(
                "Provider model_metadata cost values must be non-negative",
            ));
        }
        validate_runtime_cost_tiers(&metadata.cost_tiers)?;
        validate_json_object(&metadata.compat, "Provider model_metadata compat")?;
        validate_string_dict(&metadata.headers, "Provider model_metadata headers")?;
        for value in metadata.thinking_level_map.values() {
            if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
                return Err(ProviderConfigError::new(
                    "Provider model_metadata thinking_level_map values must be strings or null",
                ));
            }
        }
    }
    Ok(())
}

fn validate_runtime_cost_tiers(tiers: &[ModelCostTier]) -> Result<(), ProviderConfigError> {
    if tiers
        .last()
        .is_some_and(|tier| tier.max_input_tokens.is_some())
    {
        return Err(ProviderConfigError::new(
            "Provider model_metadata final cost tier must omit max_input_tokens",
        ));
    }

    let mut previous_limit = 0;
    for tier in tiers {
        if tier.cost.values().any(|value| *value < 0.0) {
            return Err(ProviderConfigError::new(
                "Provider model_metadata cost tier values must be non-negative",
            ));
        }
        let Some(limit) = tier.max_input_tokens else {
            continue;
        };
        if limit <= previous_limit {
            return Err(ProviderConfigError::new(
                "Provider model_metadata cost tier limits must be strictly increasing",
            ));
        }
        previous_limit = limit;
    }
    Ok(())
}

fn validate_string_dict(
    value: &BTreeMap<String, String>,
    field_name: &str,
) -> Result<(), ProviderConfigError> {
    for (key, item) in value {
        if key.trim().is_empty() {
            return Err(ProviderConfigError::new(format!(
                "{field_name} keys must be non-empty strings"
            )));
        }
        if item.trim().is_empty() {
            return Err(ProviderConfigError::new(format!(
                "{field_name} values must be non-empty strings"
            )));
        }
    }
    Ok(())
}

fn validate_json_object(
    value: &BTreeMap<String, JsonValue>,
    field_name: &str,
) -> Result<(), ProviderConfigError> {
    for (key, item) in value {
        if key.trim().is_empty() {
            return Err(ProviderConfigError::new(format!(
                "{field_name} keys must be non-empty strings"
            )));
        }
        validate_json_value(item, &format!("{field_name}.{key}"))?;
    }
    Ok(())
}

fn validate_json_value(value: &JsonValue, field_name: &str) -> Result<(), ProviderConfigError> {
    match value {
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
            Ok(())
        }
        JsonValue::Array(items) => {
            for item in items {
                validate_json_value(item, field_name)?;
            }
            Ok(())
        }
        JsonValue::Object(entries) => {
            for (key, item) in entries {
                validate_json_value(item, &format!("{field_name}.{key}"))?;
            }
            Ok(())
        }
    }
}

fn reject_codex_legacy_compat(
    compat: &BTreeMap<String, JsonValue>,
) -> Result<(), ProviderConfigError> {
    if compat.is_empty() {
        return Ok(());
    }
    Err(ProviderConfigError::new(
        "OpenAI Codex legacy provider compat is not supported",
    ))
}

fn validate_inference_providers(
    provider_name: &str,
    models: &[String],
    inference_providers: &BTreeMap<String, String>,
) -> Result<(), ProviderConfigError> {
    if !inference_providers.is_empty() && provider_name != "huggingface" {
        return Err(ProviderConfigError::new(
            "inference_providers preferences are only supported for the huggingface provider",
        ));
    }
    for (model, route) in inference_providers {
        if !models.iter().any(|declared| declared == model) {
            return Err(ProviderConfigError::new(format!(
                "Inference-provider preference references unknown model: {model}"
            )));
        }
        validate_huggingface_inference_provider(route)?;
    }
    Ok(())
}

fn validate_thinking_defaults(
    thinking_defaults: &BTreeMap<String, ThinkingLevel>,
) -> Result<(), ProviderConfigError> {
    for model in thinking_defaults.keys() {
        if model.trim().is_empty() {
            return Err(ProviderConfigError::new(
                "Provider thinking_defaults keys must be non-empty strings",
            ));
        }
    }
    Ok(())
}

fn validate_thinking_config(
    thinking_levels: Option<&[ThinkingLevel]>,
    thinking_models: &[String],
    thinking_default: Option<ThinkingLevel>,
    thinking_parameter: Option<ThinkingParameter>,
) -> Result<(), ProviderConfigError> {
    let Some(levels) = thinking_levels else {
        if !thinking_models.is_empty() || thinking_default.is_some() || thinking_parameter.is_some()
        {
            return Err(ProviderConfigError::new(
                "Provider thinking_levels must be set before thinking metadata",
            ));
        }
        return Ok(());
    };

    if thinking_models.iter().any(|model| model.trim().is_empty()) {
        return Err(ProviderConfigError::new(
            "Provider thinking_models must contain non-empty strings",
        ));
    }
    if thinking_default.is_some_and(|default| !levels.contains(&default)) {
        return Err(ProviderConfigError::new(
            "Provider thinking_default must be in thinking_levels",
        ));
    }
    Ok(())
}

fn levels_from_thinking_map(
    thinking_level_map: &BTreeMap<ThinkingLevel, Option<String>>,
) -> Vec<ThinkingLevel> {
    ThinkingLevel::ALL
        .into_iter()
        .filter(|level| thinking_level_map_supports(thinking_level_map, *level))
        .collect()
}

fn metadata_supports_thinking_level(
    metadata: &ProviderModelMetadata,
    level: ThinkingLevel,
) -> bool {
    if metadata.reasoning.is_none() && metadata.thinking_level_map.is_empty() {
        return true;
    }
    thinking_level_map_supports(&metadata.thinking_level_map, level)
}

fn thinking_level_map_supports(
    thinking_level_map: &BTreeMap<ThinkingLevel, Option<String>>,
    level: ThinkingLevel,
) -> bool {
    match thinking_level_map.get(&level) {
        Some(value) => value.is_some(),
        None => !matches!(level, ThinkingLevel::Xhigh | ThinkingLevel::Max),
    }
}

fn provider_preference_to_json(provider: &ProviderConfig) -> JsonValue {
    let mut preference: Map<String, JsonValue> = Map::new();
    preference.insert(
        "default_model".to_string(),
        JsonValue::from(provider.default_model().to_string()),
    );
    preference.insert("headers".to_string(), json_string_map(provider.headers()));
    preference.insert(
        "timeout_seconds".to_string(),
        json_number(provider.timeout_seconds()),
    );
    preference.insert(
        "max_retries".to_string(),
        JsonValue::from(provider.max_retries()),
    );
    preference.insert(
        "max_retry_delay_seconds".to_string(),
        json_number(provider.max_retry_delay_seconds()),
    );
    preference.insert(
        "thinking_defaults".to_string(),
        json_thinking_defaults(provider.thinking_defaults()),
    );
    if let ProviderConfig::OpenAiCompatible(config) = provider
        && !config.inference_providers.is_empty()
    {
        preference.insert(
            "inference_providers".to_string(),
            json_string_map(&config.inference_providers),
        );
    }
    JsonValue::Object(preference)
}

fn string_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<String, ProviderConfigError> {
    match value {
        Some(JsonValue::String(text)) if !text.trim().is_empty() => Ok(text.trim().to_string()),
        _ => Err(ProviderConfigError::new(format!(
            "Provider field must be a non-empty string: {field_name}"
        ))),
    }
}

fn optional_string_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Option<String>, ProviderConfigError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(text)) if !text.trim().is_empty() => {
            Ok(Some(text.trim().to_string()))
        }
        _ => Err(ProviderConfigError::new(format!(
            "Provider field must be a non-empty string: {field_name}"
        ))),
    }
}

fn string_tuple_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Vec<String>, ProviderConfigError> {
    match value {
        Some(JsonValue::Array(items)) if !items.is_empty() => parse_string_items(items, field_name),
        _ => Err(ProviderConfigError::new(format!(
            "Provider field must be a non-empty string list: {field_name}"
        ))),
    }
}

fn optional_string_tuple_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Vec<String>, ProviderConfigError> {
    match value {
        None => Ok(Vec::new()),
        Some(JsonValue::Array(items)) => parse_string_items(items, field_name),
        _ => Err(ProviderConfigError::new(format!(
            "Provider field must be a string list: {field_name}"
        ))),
    }
}

fn parse_string_items(
    items: &[JsonValue],
    field_name: &str,
) -> Result<Vec<String>, ProviderConfigError> {
    let mut parsed = Vec::with_capacity(items.len());
    for item in items {
        match item {
            JsonValue::String(text) if !text.trim().is_empty() => {
                parsed.push(text.trim().to_string());
            }
            _ => {
                return Err(ProviderConfigError::new(format!(
                    "Provider field must be a string list: {field_name}"
                )));
            }
        }
    }
    Ok(parsed)
}

fn optional_thinking_levels_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Option<Vec<ThinkingLevel>>, ProviderConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let JsonValue::Array(items) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be a thinking mode list: {field_name}"
        )));
    };

    let raw = parse_string_items(items, field_name)?;
    normalize_thinking_levels(&raw).map(Some)
}

fn optional_thinking_level_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Option<ThinkingLevel>, ProviderConfigError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(text)) => normalize_thinking_level(Some(text)).map(Some),
        _ => Err(ProviderConfigError::new(format!(
            "Provider field must be a thinking mode: {field_name}"
        ))),
    }
}

fn optional_thinking_parameter_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Option<ThinkingParameter>, ProviderConfigError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(text)) => {
            ThinkingParameter::parse(text).map(Some).ok_or_else(|| {
                ProviderConfigError::new(format!(
                    "Provider field must be reasoning_effort, reasoning.effort, \
                 or anthropic.thinking: {field_name}"
                ))
            })
        }
        _ => Err(ProviderConfigError::new(format!(
            "Provider field must be reasoning_effort, reasoning.effort, \
             or anthropic.thinking: {field_name}"
        ))),
    }
}

fn optional_provider_api_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Option<ProviderApi>, ProviderConfigError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::String(text)) => ProviderApi::parse(text).map(Some).ok_or_else(|| {
            ProviderConfigError::new(format!("Provider field has unsupported API: {field_name}"))
        }),
        _ => Err(ProviderConfigError::new(format!(
            "Provider field has unsupported API: {field_name}"
        ))),
    }
}

fn string_dict_field(
    value: &JsonValue,
    field_name: &str,
) -> Result<BTreeMap<String, String>, ProviderConfigError> {
    let JsonValue::Object(entries) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be a string object: {field_name}"
        )));
    };

    let mut parsed: BTreeMap<String, String> = BTreeMap::new();
    for (key, item) in entries {
        let JsonValue::String(text) = item else {
            return Err(ProviderConfigError::new(format!(
                "Provider field must be a string object: {field_name}"
            )));
        };
        if key.trim().is_empty() || text.trim().is_empty() {
            return Err(ProviderConfigError::new(format!(
                "Provider field must be a string object: {field_name}"
            )));
        }
        parsed.insert(key.trim().to_string(), text.trim().to_string());
    }
    Ok(parsed)
}

fn json_dict_field(
    value: &JsonValue,
    field_name: &str,
) -> Result<BTreeMap<String, JsonValue>, ProviderConfigError> {
    let JsonValue::Object(entries) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be an object: {field_name}"
        )));
    };

    let mut parsed: BTreeMap<String, JsonValue> = BTreeMap::new();
    for (key, item) in entries {
        if key.trim().is_empty() {
            return Err(ProviderConfigError::new(format!(
                "Provider field must have string keys: {field_name}"
            )));
        }
        validate_json_value(item, &format!("{field_name}.{key}"))?;
        parsed.insert(key.trim().to_string(), item.clone());
    }
    Ok(parsed)
}

fn float_dict_field(
    value: &JsonValue,
    field_name: &str,
) -> Result<BTreeMap<String, f64>, ProviderConfigError> {
    let JsonValue::Object(entries) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be a number object: {field_name}"
        )));
    };

    let mut parsed: BTreeMap<String, f64> = BTreeMap::new();
    for (key, item) in entries {
        if key.trim().is_empty() {
            return Err(ProviderConfigError::new(format!(
                "Provider field must be a number object: {field_name}"
            )));
        }
        let Some(number) = json_as_number(item) else {
            return Err(ProviderConfigError::new(format!(
                "Provider field values must be non-negative: {field_name}"
            )));
        };
        if number < 0.0 {
            return Err(ProviderConfigError::new(format!(
                "Provider field values must be non-negative: {field_name}"
            )));
        }
        parsed.insert(key.trim().to_string(), number);
    }
    Ok(parsed)
}

fn context_window_dict_field(
    value: &JsonValue,
    field_name: &str,
) -> Result<BTreeMap<String, i64>, ProviderConfigError> {
    let JsonValue::Object(entries) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be an integer object: {field_name}"
        )));
    };

    let mut parsed: BTreeMap<String, i64> = BTreeMap::new();
    for (key, item) in entries {
        if key.trim().is_empty() {
            return Err(ProviderConfigError::new(format!(
                "Provider field must be an integer object: {field_name}"
            )));
        }
        let Some(window) = json_as_integer(item) else {
            return Err(ProviderConfigError::new(format!(
                "Provider field values must be positive integers: {field_name}"
            )));
        };
        if window <= 0 {
            return Err(ProviderConfigError::new(format!(
                "Provider field values must be positive integers: {field_name}"
            )));
        }
        parsed.insert(key.trim().to_string(), window);
    }
    Ok(parsed)
}

fn model_metadata_dict_field(
    value: &JsonValue,
    models: &[String],
    field_name: &str,
) -> Result<BTreeMap<String, ProviderModelMetadata>, ProviderConfigError> {
    let JsonValue::Object(entries) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be an object: {field_name}"
        )));
    };

    let mut parsed: BTreeMap<String, ProviderModelMetadata> = BTreeMap::new();
    for (key, item) in entries {
        let model = string_field(Some(&JsonValue::String(key.clone())), field_name)?;
        if !models.iter().any(|declared| declared == &model) {
            return Err(ProviderConfigError::new(format!(
                "Provider model_metadata key is not in models: {model}"
            )));
        }
        let JsonValue::Object(metadata) = item else {
            return Err(ProviderConfigError::new(format!(
                "Provider model_metadata entries must be objects: {field_name}"
            )));
        };
        parsed.insert(
            model.clone(),
            model_metadata_field(metadata, field_name, &model)?,
        );
    }
    Ok(parsed)
}

fn model_metadata_field(
    metadata: &Map<String, JsonValue>,
    field_name: &str,
    model: &str,
) -> Result<ProviderModelMetadata, ProviderConfigError> {
    let field = |field: &str| format!("{field_name}.{model}.{field}");
    let empty = empty_object();

    Ok(ProviderModelMetadata {
        name: optional_string_field(metadata.get("name"), &field("name"))?,
        api: optional_provider_api_field(metadata.get("api"), &field("api"))?,
        base_url: optional_string_field(metadata.get("base_url"), &field("base_url"))?,
        reasoning: optional_bool_field(metadata.get("reasoning"), &field("reasoning"))?,
        input: optional_string_tuple_field(metadata.get("input"), &field("input"))?,
        cost: float_dict_field(metadata.get("cost").unwrap_or(&empty), &field("cost"))?,
        cost_tiers: cost_tiers_field(
            metadata
                .get("cost_tiers")
                .unwrap_or(&JsonValue::Array(Vec::new())),
            &field("cost_tiers"),
        )?,
        context_window: optional_positive_int_field(
            metadata.get("context_window"),
            &field("context_window"),
        )?,
        max_tokens: optional_positive_int_field(metadata.get("max_tokens"), &field("max_tokens"))?,
        headers: string_dict_field(metadata.get("headers").unwrap_or(&empty), &field("headers"))?,
        compat: json_dict_field(metadata.get("compat").unwrap_or(&empty), &field("compat"))?,
        thinking_level_map: thinking_level_map_field(
            metadata.get("thinking_level_map").unwrap_or(&empty),
            &field("thinking_level_map"),
        )?,
    })
}

fn cost_tiers_field(
    value: &JsonValue,
    field_name: &str,
) -> Result<Vec<ModelCostTier>, ProviderConfigError> {
    let JsonValue::Array(entries) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be an array: {field_name}"
        )));
    };

    let allowed = [
        "max_input_tokens",
        "input",
        "output",
        "cacheRead",
        "cacheWrite",
        "cacheWrite1h",
    ];
    let mut tiers = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let JsonValue::Object(tier) = entry else {
            return Err(ProviderConfigError::new(format!(
                "Provider cost tiers must be objects: {field_name}"
            )));
        };
        let tier_field = format!("{field_name}.{index}");
        if tier.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(ProviderConfigError::new(format!(
                "Provider cost tier has unknown fields: {tier_field}"
            )));
        }

        let mut cost: BTreeMap<String, f64> = BTreeMap::new();
        for key in ["input", "output", "cacheRead", "cacheWrite"] {
            cost.insert(
                key.to_string(),
                non_negative_float_field(
                    tier.get(key).unwrap_or(&JsonValue::Null),
                    &format!("{tier_field}.{key}"),
                )?,
            );
        }
        if let Some(raw) = tier.get("cacheWrite1h")
            && raw != &JsonValue::Null
        {
            cost.insert(
                "cacheWrite1h".to_string(),
                non_negative_float_field(raw, &format!("{tier_field}.cacheWrite1h"))?,
            );
        }

        tiers.push(ModelCostTier {
            cost,
            max_input_tokens: optional_positive_int_field(
                tier.get("max_input_tokens"),
                &format!("{tier_field}.max_input_tokens"),
            )?,
        });
    }

    validate_runtime_cost_tiers(&tiers)?;
    Ok(tiers)
}

fn thinking_level_map_field(
    value: &JsonValue,
    field_name: &str,
) -> Result<BTreeMap<ThinkingLevel, Option<String>>, ProviderConfigError> {
    let JsonValue::Object(entries) = value else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be an object: {field_name}"
        )));
    };

    let mut parsed: BTreeMap<ThinkingLevel, Option<String>> = BTreeMap::new();
    for (key, item) in entries {
        let level = normalize_thinking_level(Some(key.as_str()))?;
        let mapped = match item {
            JsonValue::Null => None,
            JsonValue::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
            _ => {
                return Err(ProviderConfigError::new(format!(
                    "Provider field values must be strings or null: {field_name}"
                )));
            }
        };
        parsed.insert(level, mapped);
    }
    Ok(parsed)
}

fn optional_bool_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Option<bool>, ProviderConfigError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Bool(flag)) => Ok(Some(*flag)),
        _ => Err(ProviderConfigError::new(format!(
            "Provider field must be a boolean: {field_name}"
        ))),
    }
}

fn optional_positive_int_field(
    value: Option<&JsonValue>,
    field_name: &str,
) -> Result<Option<i64>, ProviderConfigError> {
    match value {
        None | Some(JsonValue::Null) => Ok(None),
        Some(item) => match json_as_integer(item) {
            Some(number) if number > 0 => Ok(Some(number)),
            _ => Err(ProviderConfigError::new(format!(
                "Provider field must be a positive integer: {field_name}"
            ))),
        },
    }
}

fn positive_float_field(value: &JsonValue, field_name: &str) -> Result<f64, ProviderConfigError> {
    let Some(number) = json_as_number(value) else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be a positive number: {field_name}"
        )));
    };
    if number <= 0.0 {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be greater than 0: {field_name}"
        )));
    }
    Ok(number)
}

fn non_negative_int_field(value: &JsonValue, field_name: &str) -> Result<i64, ProviderConfigError> {
    let Some(number) = json_as_integer(value) else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be a non-negative integer: {field_name}"
        )));
    };
    if number < 0 {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be 0 or greater: {field_name}"
        )));
    }
    Ok(number)
}

fn non_negative_float_field(
    value: &JsonValue,
    field_name: &str,
) -> Result<f64, ProviderConfigError> {
    let Some(number) = json_as_number(value) else {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be a non-negative number: {field_name}"
        )));
    };
    if number < 0.0 {
        return Err(ProviderConfigError::new(format!(
            "Provider field must be 0 or greater: {field_name}"
        )));
    }
    Ok(number)
}

/// A JSON number as a float, rejecting booleans the way Python's `isinstance`
/// check does.
fn json_as_number(value: &JsonValue) -> Option<f64> {
    match value {
        JsonValue::Number(number) => number.as_f64(),
        _ => None,
    }
}

/// A JSON number as an integer, rejecting floats and booleans.
fn json_as_integer(value: &JsonValue) -> Option<i64> {
    match value {
        JsonValue::Number(number) => number.as_i64(),
        _ => None,
    }
}

fn python_repr(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => format!("'{text}'"),
        JsonValue::Bool(true) => "True".to_string(),
        JsonValue::Bool(false) => "False".to_string(),
        JsonValue::Null => "None".to_string(),
        other => other.to_string(),
    }
}

fn empty_object() -> JsonValue {
    JsonValue::Object(Map::new())
}

fn json_number(value: f64) -> JsonValue {
    serde_json::Number::from_f64(value).map_or(JsonValue::Null, JsonValue::Number)
}

fn json_optional_string(value: Option<&str>) -> JsonValue {
    value.map_or(JsonValue::Null, JsonValue::from)
}

fn json_strings(values: &[String]) -> JsonValue {
    JsonValue::Array(
        values
            .iter()
            .map(|value| JsonValue::from(value.as_str()))
            .collect(),
    )
}

fn json_number_map(values: &BTreeMap<String, f64>) -> JsonValue {
    let object: Map<String, JsonValue> = values
        .iter()
        .map(|(key, value)| (key.clone(), json_number(*value)))
        .collect();
    JsonValue::Object(object)
}

fn json_string_map(values: &BTreeMap<String, String>) -> JsonValue {
    let object: Map<String, JsonValue> = values
        .iter()
        .map(|(key, value)| (key.clone(), JsonValue::from(value.clone())))
        .collect();
    JsonValue::Object(object)
}

fn json_value_map(values: &BTreeMap<String, JsonValue>) -> JsonValue {
    let object: Map<String, JsonValue> = values
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    JsonValue::Object(object)
}

fn json_thinking_defaults(values: &BTreeMap<String, ThinkingLevel>) -> JsonValue {
    let object: Map<String, JsonValue> = values
        .iter()
        .map(|(key, value)| (key.clone(), JsonValue::from(value.as_str())))
        .collect();
    JsonValue::Object(object)
}

fn json_thinking_level_map(values: &BTreeMap<ThinkingLevel, Option<String>>) -> JsonValue {
    let object: Map<String, JsonValue> = values
        .iter()
        .map(|(level, value)| {
            (
                level.as_str().to_string(),
                value
                    .as_ref()
                    .map_or(JsonValue::Null, |value| JsonValue::from(value.clone())),
            )
        })
        .collect();
    JsonValue::Object(object)
}

fn insert_provider_common_json(object: &mut Map<String, JsonValue>, credential_name: Option<&str>) {
    object.insert(
        "credential_name".to_string(),
        credential_name.map_or(JsonValue::Null, JsonValue::from),
    );
}

fn insert_model_json(
    object: &mut Map<String, JsonValue>,
    models: &[String],
    default_model: &str,
    context_windows: &BTreeMap<String, i64>,
    headers: &BTreeMap<String, String>,
    model_metadata: &BTreeMap<String, ProviderModelMetadata>,
) {
    object.insert("models".to_string(), json_strings(models));
    object.insert(
        "default_model".to_string(),
        JsonValue::from(default_model.to_string()),
    );
    object.insert(
        "context_windows".to_string(),
        JsonValue::Object(
            context_windows
                .iter()
                .map(|(key, value)| (key.clone(), JsonValue::from(*value)))
                .collect(),
        ),
    );
    object.insert("headers".to_string(), json_string_map(headers));
    object.insert(
        "model_metadata".to_string(),
        JsonValue::Object(
            model_metadata
                .iter()
                .map(|(key, value)| (key.clone(), value.to_json()))
                .collect(),
        ),
    );
}

fn insert_provider_numbers_json(
    object: &mut Map<String, JsonValue>,
    timeout_seconds: f64,
    max_retries: i64,
    max_retry_delay_seconds: f64,
) {
    object.insert("timeout_seconds".to_string(), json_number(timeout_seconds));
    object.insert("max_retries".to_string(), JsonValue::from(max_retries));
    object.insert(
        "max_retry_delay_seconds".to_string(),
        json_number(max_retry_delay_seconds),
    );
}

fn insert_thinking_json(
    object: &mut Map<String, JsonValue>,
    thinking_levels: Option<&[ThinkingLevel]>,
    thinking_models: &[String],
    thinking_default: Option<ThinkingLevel>,
    thinking_parameter: Option<ThinkingParameter>,
    thinking_defaults: &BTreeMap<String, ThinkingLevel>,
) {
    object.insert(
        "thinking_levels".to_string(),
        thinking_levels.map_or(JsonValue::Null, |levels| {
            JsonValue::Array(
                levels
                    .iter()
                    .map(|level| JsonValue::from(level.as_str()))
                    .collect(),
            )
        }),
    );
    object.insert("thinking_models".to_string(), json_strings(thinking_models));
    object.insert(
        "thinking_default".to_string(),
        thinking_default.map_or(JsonValue::Null, |level| JsonValue::from(level.as_str())),
    );
    object.insert(
        "thinking_parameter".to_string(),
        thinking_parameter.map_or(JsonValue::Null, |parameter| {
            JsonValue::from(parameter.as_str())
        }),
    );
    object.insert(
        "thinking_defaults".to_string(),
        json_thinking_defaults(thinking_defaults),
    );
}
