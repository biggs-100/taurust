//! Durable provider configuration: what `providers.json` carries and in which
//! order a credential or an environment value wins.
//!
//! Mirrors tau's `tests/test_provider_config.py`, restricted to the surface the
//! module ports: parsing, validation and resolution. The reference tests drive
//! `openai_compatible_config_from_provider` and `anthropic_config_from_provider`
//! for the credential and base-URL cases; those two return runtime adapter
//! configuration owned by `tau_ai`, so the same assertions are made against the
//! resolution functions that feed them (`api_key_from_provider`,
//! `openai_compatible_base_url`). See the module doc comment for why.
//!
//! The reference monkeypatches `os.environ`; these read a `MapEnv`.

use std::collections::BTreeMap;

use tau_ai::env::MapEnv;
use tau_coding::paths::TauPaths;
use tau_coding::provider_config::{
    AnthropicProviderConfig, CredentialReader, DEFAULT_MODEL, DEFAULT_PROVIDER_NAME, ModelCostTier,
    OpenAiCompatibleProviderConfig, ProviderConfig, ProviderModelMetadata, ProviderSettings,
    ScopedModelConfig, ThinkingLevel, ThinkingParameter, api_key_from_provider,
    load_provider_settings, openai_compatible_base_url, provider_default_thinking_level,
    provider_has_usable_credentials, provider_model_max_tokens, provider_model_supports_images,
    provider_settings_from_json, provider_thinking_levels, resolve_provider_selection,
    validate_huggingface_inference_provider,
};
use tau_protocol::JsonValue;

fn openai() -> ProviderConfig {
    ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        models: vec!["gpt-5.4".to_string(), "gpt-5-mini".to_string()],
        ..OpenAiCompatibleProviderConfig::new("openai")
    })
}

fn huggingface() -> ProviderConfig {
    ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://router.huggingface.co/v1".to_string(),
        api_key_env: "HF_TOKEN".to_string(),
        credential_name: Some("huggingface".to_string()),
        models: vec![
            "zai-org/GLM-5.2".to_string(),
            "moonshotai/Kimi-K3".to_string(),
        ],
        default_model: "zai-org/GLM-5.2".to_string(),
        ..OpenAiCompatibleProviderConfig::new("huggingface")
    })
}

fn catalog() -> Vec<ProviderConfig> {
    vec![
        openai(),
        ProviderConfig::Anthropic(AnthropicProviderConfig::new("anthropic")),
        huggingface(),
    ]
}

fn settings_paths(directory: &std::path::Path) -> TauPaths {
    TauPaths::new(directory.join(".tau"), directory.join(".agents"))
}

fn openai_config(source: &str) -> ProviderConfig {
    ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        models: vec!["gpt-5.4".to_string()],
        ..OpenAiCompatibleProviderConfig::new(source)
    })
}

struct StoredKey(&'static str, &'static str);

impl CredentialReader for StoredKey {
    fn get(&self, name: &str) -> Option<String> {
        (name == self.0).then(|| self.1.to_string())
    }
}

struct NoCredentials;

impl CredentialReader for NoCredentials {
    fn get(&self, _name: &str) -> Option<String> {
        None
    }
}

#[test]
fn an_unknown_schema_version_is_rejected() {
    let error = provider_settings_from_json(
        &serde_json::json!({
            "schema_version": 99,
            "default_provider": "openai",
            "provider_preferences": {},
        }),
        &catalog(),
    )
    .expect_err("an unknown schema version is not readable");

    assert_eq!(
        error.message(),
        "Unsupported provider settings schema_version: 99"
    );
}

#[test]
fn a_settings_file_that_is_not_an_object_is_rejected() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let paths = settings_paths(directory.path());
    std::fs::create_dir_all(paths.home()).expect("the home directory is created");
    std::fs::write(paths.home().join("providers.json"), "[]").expect("the file is written");

    let error =
        load_provider_settings(&paths, &catalog()).expect_err("a list is not a settings object");

    assert_eq!(error.message(), "Provider settings must be a JSON object");
}

#[test]
fn a_missing_settings_file_uses_the_catalog_defaults() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let paths = settings_paths(directory.path());

    let settings = load_provider_settings(&paths, &catalog()).expect("the defaults resolve");

    assert_eq!(settings.default_provider, DEFAULT_PROVIDER_NAME);
    assert_eq!(
        settings
            .providers
            .iter()
            .map(|provider| provider.name())
            .collect::<Vec<_>>(),
        vec!["openai", "anthropic", "huggingface"]
    );
    assert_eq!(
        settings
            .get_provider(None)
            .expect("the default provider exists")
            .default_model(),
        DEFAULT_MODEL
    );
    assert_eq!(
        settings
            .get_provider(Some("anthropic"))
            .expect("anthropic exists")
            .api_key_env(),
        "ANTHROPIC_API_KEY"
    );
    assert_eq!(
        settings
            .get_provider(Some("huggingface"))
            .expect("huggingface exists")
            .api_key_env(),
        "HF_TOKEN"
    );
}

#[test]
fn unknown_top_level_and_preference_fields_are_ignored() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "openai",
            "future_top_level_option": true,
            "provider_preferences": {
                "openai": {
                    "default_model": "gpt-5-mini",
                    "future_provider_option": {"enabled": true},
                },
            },
        }),
        &catalog(),
    )
    .expect("unknown fields are tolerated");

    assert_eq!(settings.default_provider, "openai");
    assert_eq!(
        settings
            .get_provider(Some("openai"))
            .expect("openai exists")
            .default_model(),
        "gpt-5-mini"
    );
    assert_eq!(
        settings.providers.len(),
        1,
        "only the preferred provider is carried by the parsed settings"
    );
}

#[test]
fn a_preference_without_a_catalog_entry_is_ignored() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let paths = settings_paths(directory.path());
    std::fs::create_dir_all(paths.home()).expect("the home directory is created");
    std::fs::write(
        paths.home().join("providers.json"),
        serde_json::json!({
            "default_provider": "openai",
            "provider_preferences": {
                "openai": {"default_model": "gpt-5-mini"},
                "llama-cpp": {"default_model": "local"},
            },
        })
        .to_string(),
    )
    .expect("the file is written");

    let settings = load_provider_settings(&paths, &catalog()).expect("the settings load");

    assert_eq!(
        settings
            .get_provider(Some("openai"))
            .expect("openai exists")
            .default_model(),
        "gpt-5-mini"
    );
    assert!(
        settings
            .providers
            .iter()
            .all(|provider| provider.name() != "llama-cpp"),
        "an orphaned preference does not resurrect a provider"
    );
}

#[test]
fn preferences_override_the_catalog_runtime_values() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let paths = settings_paths(directory.path());
    std::fs::create_dir_all(paths.home()).expect("the home directory is created");
    std::fs::write(
        paths.home().join("providers.json"),
        serde_json::json!({
            "default_provider": "openai",
            "provider_preferences": {
                "openai": {
                    "default_model": "gpt-5-mini",
                    "headers": {"X-Test": "yes"},
                    "timeout_seconds": 12.0,
                    "max_retries": 1,
                    "max_retry_delay_seconds": 0.5,
                    "thinking_defaults": {},
                },
            },
            "scoped_models": [{"provider": "openai", "model": "gpt-5-mini"}],
        })
        .to_string(),
    )
    .expect("the file is written");

    let settings = load_provider_settings(&paths, &catalog()).expect("the settings load");
    let provider = settings
        .get_provider(Some("openai"))
        .expect("openai exists");

    assert_eq!(provider.default_model(), "gpt-5-mini");
    assert_eq!(
        provider.headers().get("X-Test").map(String::as_str),
        Some("yes")
    );
    assert_eq!(provider.timeout_seconds(), 12.0);
    assert_eq!(provider.max_retries(), 1);
    assert_eq!(provider.max_retry_delay_seconds(), 0.5);
    assert_eq!(
        settings.scoped_models,
        vec![ScopedModelConfig::new("openai", "gpt-5-mini")]
    );
}

#[test]
fn a_preference_default_model_outside_the_declared_models_is_ignored() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "openai",
            "provider_preferences": {"openai": {"default_model": "gpt-9-unknown"}},
        }),
        &catalog(),
    )
    .expect("the preference is read");

    assert_eq!(
        settings
            .get_provider(Some("openai"))
            .expect("openai exists")
            .default_model(),
        DEFAULT_MODEL,
        "an unknown model cannot become the default"
    );
}

#[test]
fn preferences_must_be_an_object() {
    let error = provider_settings_from_json(
        &serde_json::json!({"default_provider": "openai", "provider_preferences": []}),
        &catalog(),
    )
    .expect_err("a list is not a preference map");

    assert_eq!(
        error.message(),
        "Provider settings field must be an object: provider_preferences"
    );
}

#[test]
fn a_preference_entry_must_be_an_object() {
    let error = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "openai",
            "provider_preferences": {"openai": 7},
        }),
        &catalog(),
    )
    .expect_err("a number is not a preference entry");

    assert_eq!(
        error.message(),
        "Provider preference entries must be objects"
    );
}

#[test]
fn huggingface_inference_provider_preferences_load() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "huggingface",
            "provider_preferences": {
                "huggingface": {
                    "default_model": "zai-org/GLM-5.2",
                    "inference_providers": {"zai-org/GLM-5.2": "deepinfra"},
                },
            },
        }),
        &catalog(),
    )
    .expect("the preference is read");
    let provider = settings
        .get_provider(Some("huggingface"))
        .expect("huggingface exists");

    let ProviderConfig::OpenAiCompatible(config) = provider else {
        panic!("huggingface is OpenAI-compatible");
    };
    assert_eq!(
        config
            .inference_providers
            .get("zai-org/GLM-5.2")
            .map(String::as_str),
        Some("deepinfra")
    );
}

#[test]
fn legacy_provider_list_loads_headers() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "huggingface",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "huggingface",
                    "base_url": "https://router.huggingface.co/v1",
                    "api_key_env": "HF_TOKEN",
                    "credential_name": "huggingface",
                    "models": ["Qwen/Qwen3-Coder"],
                    "default_model": "Qwen/Qwen3-Coder",
                    "headers": {"X-HF-Bill-To": "my-org"},
                },
            ],
        }),
        &catalog(),
    )
    .expect("the legacy list is read");
    let provider = settings
        .get_provider(Some("huggingface"))
        .expect("huggingface exists");

    assert!(matches!(provider, ProviderConfig::OpenAiCompatible(_)));
    assert_eq!(
        provider.headers().get("X-HF-Bill-To").map(String::as_str),
        Some("my-org")
    );
}

#[test]
fn legacy_provider_list_loads_custom_thinking_capabilities() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["reasoner", "plain"],
                    "default_model": "reasoner",
                    "thinking_levels": ["off", "low", "high"],
                    "thinking_models": ["reasoner"],
                    "thinking_default": "low",
                    "thinking_parameter": "reasoning_effort",
                    "thinking_defaults": {"reasoner": "high"},
                },
            ],
        }),
        &catalog(),
    )
    .expect("the legacy list is read");
    let provider = settings.get_provider(Some("local")).expect("local exists");

    assert_eq!(
        provider_thinking_levels(provider, Some("reasoner")),
        vec![ThinkingLevel::Off, ThinkingLevel::Low, ThinkingLevel::High]
    );
    assert_eq!(
        provider_thinking_levels(provider, Some("plain")),
        Vec::<ThinkingLevel>::new()
    );
    assert_eq!(
        provider_default_thinking_level(provider, Some("reasoner")),
        Some(ThinkingLevel::Low)
    );
    let ProviderConfig::OpenAiCompatible(config) = provider else {
        panic!("local is OpenAI-compatible");
    };
    assert_eq!(
        config.thinking_defaults.get("reasoner"),
        Some(&ThinkingLevel::High)
    );
    assert_eq!(
        provider.to_json()["thinking_parameter"],
        serde_json::json!("reasoning_effort")
    );
}

#[test]
fn legacy_provider_list_loads_an_anthropic_thinking_provider() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "anthropic",
            "providers": [
                {
                    "type": "anthropic",
                    "name": "anthropic",
                    "base_url": "https://api.anthropic.com/v1",
                    "api_key_env": "ANTHROPIC_API_KEY",
                    "models": ["claude-sonnet-4-6"],
                    "default_model": "claude-sonnet-4-6",
                    "thinking_levels": ["off", "low", "high"],
                    "thinking_models": ["claude-sonnet-4-6"],
                    "thinking_parameter": "anthropic.thinking",
                },
            ],
        }),
        &catalog(),
    )
    .expect("the anthropic provider is read");
    let provider = settings
        .get_provider(Some("anthropic"))
        .expect("anthropic exists");

    assert!(matches!(provider, ProviderConfig::Anthropic(_)));
    assert_eq!(
        provider_thinking_levels(provider, Some("claude-sonnet-4-6")),
        vec![ThinkingLevel::Off, ThinkingLevel::Low, ThinkingLevel::High]
    );
    assert_eq!(
        provider.thinking_parameter(),
        Some(ThinkingParameter::AnthropicThinking)
    );
}

#[test]
fn legacy_provider_list_loads_an_openai_codex_provider() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "openai-codex",
            "providers": [
                {
                    "type": "openai-codex",
                    "name": "openai-codex",
                    "base_url": "https://chatgpt.com/backend-api",
                    "api_key_env": "OPENAI_CODEX_ACCESS_TOKEN",
                    "credential_name": "openai-codex",
                    "models": ["gpt-5.5", "gpt-5.4"],
                    "default_model": "gpt-5.5",
                    "headers": {"X-Test": "enabled"},
                },
            ],
        }),
        &catalog(),
    )
    .expect("the codex provider is read");
    let provider = settings
        .get_provider(Some("openai-codex"))
        .expect("openai-codex exists");

    assert!(matches!(provider, ProviderConfig::OpenAiCodex(_)));
    assert_eq!(provider.default_model(), "gpt-5.5");
    assert_eq!(
        provider.headers().get("X-Test").map(String::as_str),
        Some("enabled")
    );
}

#[test]
fn a_legacy_codex_provider_rejects_compat() {
    let error = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "openai-codex",
            "providers": [
                {
                    "type": "openai-codex",
                    "name": "openai-codex",
                    "base_url": "https://chatgpt.com/backend-api",
                    "api_key_env": "OPENAI_CODEX_ACCESS_TOKEN",
                    "models": ["gpt-5.5"],
                    "default_model": "gpt-5.5",
                    "compat": {"supportsStore": true},
                },
            ],
        }),
        &catalog(),
    )
    .expect_err("legacy codex compat is not supported");

    assert_eq!(
        error.message(),
        "OpenAI Codex legacy provider compat is not supported"
    );
}

#[test]
fn an_unsupported_provider_type_is_rejected() {
    let error = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "made-up",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen"],
                    "default_model": "qwen",
                },
            ],
        }),
        &catalog(),
    )
    .expect_err("an unknown provider type is not readable");

    assert_eq!(error.message(), "Unsupported provider type: made-up");
}

#[test]
fn a_legacy_provider_model_that_is_not_declared_becomes_a_model() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen"],
                    "default_model": "llama",
                },
            ],
        }),
        &catalog(),
    )
    .expect("the list is read");
    let provider = settings.get_provider(Some("local")).expect("local exists");

    assert_eq!(provider.models(), ["qwen", "llama"]);
}

#[test]
fn legacy_provider_model_cost_tiers_round_trip() {
    let raw: JsonValue = serde_json::from_str(
        r#"{
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen"],
                    "default_model": "qwen",
                    "model_metadata": {
                        "qwen": {
                            "cost": {
                                "input": 0.3,
                                "output": 1.2,
                                "cacheRead": 0.06,
                                "cacheWrite": 0
                            },
                            "cost_tiers": [
                                {
                                    "max_input_tokens": 512000,
                                    "input": 0.3,
                                    "output": 1.2,
                                    "cacheRead": 0.06,
                                    "cacheWrite": 0,
                                    "cacheWrite1h": 0.6
                                },
                                {
                                    "input": 0.6,
                                    "output": 2.4,
                                    "cacheRead": 0.12,
                                    "cacheWrite": 0
                                }
                            ]
                        }
                    }
                }
            ],
            "scoped_models": []
        }"#,
    )
    .expect("the fixture parses");

    let settings = provider_settings_from_json(&raw, &catalog()).expect("the legacy list is read");
    let provider = settings.get_provider(Some("local")).expect("local exists");
    let metadata = &provider.model_metadata()["qwen"];

    assert_eq!(metadata.cost_tiers[0].cost.get("cacheWrite1h"), Some(&0.6));
    assert!(
        !metadata.cost_tiers[1].cost.contains_key("cacheWrite1h"),
        "a tier without the key omits it, so billing falls back to cacheWrite"
    );
    assert_eq!(
        metadata.to_json()["cost_tiers"],
        serde_json::json!([
            {
                "max_input_tokens": 512000,
                "input": 0.3,
                "output": 1.2,
                "cacheRead": 0.06,
                "cacheWrite": 0.0,
                "cacheWrite1h": 0.6
            },
            {
                "input": 0.6,
                "output": 2.4,
                "cacheRead": 0.12,
                "cacheWrite": 0.0
            }
        ]),
        "the reference treats 0 and 0.0 as equal; these floats are the written shape"
    );
}

#[test]
fn legacy_cost_tiers_reject_invalid_shapes() {
    let cases: [(JsonValue, &str); 4] = [
        (
            serde_json::json!([
                {
                    "max_input_tokens": 512000,
                    "input": 0.3,
                    "output": 1.2,
                    "cacheRead": 0.06,
                    "cacheWrite": 0
                }
            ]),
            "Provider model_metadata final cost tier must omit max_input_tokens",
        ),
        (
            serde_json::json!([
                {
                    "max_input_tokens": 512000,
                    "input": 0.3,
                    "output": 1.2,
                    "cacheRead": 0.06,
                    "cacheWrite": 0
                },
                {
                    "max_input_tokens": 400000,
                    "input": 0.4,
                    "output": 1.6,
                    "cacheRead": 0.08,
                    "cacheWrite": 0
                },
                {"input": 0.6, "output": 2.4, "cacheRead": 0.12, "cacheWrite": 0}
            ]),
            "Provider model_metadata cost tier limits must be strictly increasing",
        ),
        (
            serde_json::json!([
                {"unexpected": 1, "input": 0.3, "output": 1.2, "cacheRead": 0.06, "cacheWrite": 0}
            ]),
            "Provider cost tier has unknown fields: providers[local].model_metadata.qwen.cost_tiers.0",
        ),
        (
            serde_json::json!([
                {"input": -0.3, "output": 1.2, "cacheRead": 0.06, "cacheWrite": 0}
            ]),
            "Provider field must be 0 or greater: providers[local].model_metadata.qwen.cost_tiers.0.input",
        ),
    ];

    for (cost_tiers, expected) in cases {
        let error = provider_settings_from_json(
            &serde_json::json!({
                "default_provider": "local",
                "providers": [
                    {
                        "type": "openai-compatible",
                        "name": "local",
                        "base_url": "http://localhost:11434/v1",
                        "api_key_env": "LOCAL_API_KEY",
                        "models": ["qwen"],
                        "default_model": "qwen",
                        "model_metadata": {"qwen": {"cost_tiers": cost_tiers}},
                    },
                ],
            }),
            &catalog(),
        )
        .expect_err("the tiers are invalid");

        assert_eq!(error.message(), expected);
    }
}

#[test]
fn a_runtime_config_rejects_a_negative_cost_tier_value() {
    let config = OpenAiCompatibleProviderConfig {
        models: vec!["qwen".to_string()],
        default_model: "qwen".to_string(),
        model_metadata: BTreeMap::from([(
            "qwen".to_string(),
            ProviderModelMetadata {
                cost_tiers: vec![ModelCostTier {
                    cost: BTreeMap::from([
                        ("input".to_string(), -0.3),
                        ("output".to_string(), 1.2),
                        ("cacheRead".to_string(), 0.06),
                        ("cacheWrite".to_string(), 0.0),
                    ]),
                    max_input_tokens: None,
                }],
                ..ProviderModelMetadata::default()
            },
        )]),
        ..OpenAiCompatibleProviderConfig::new("local")
    };

    let error = config
        .validate()
        .expect_err("a negative tier value is not usable");

    assert_eq!(
        error.message(),
        "Provider model_metadata cost tier values must be non-negative"
    );
}

#[test]
fn provider_settings_parse_and_deduplicate_scoped_models() {
    let settings = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen", "llama"],
                    "default_model": "qwen",
                    "context_windows": {"qwen": 64000},
                },
            ],
            "scoped_models": [
                {"provider": "local", "model": "qwen"},
                {"provider": "local", "model": "qwen"},
                {"provider": "local", "model": "llama"},
            ],
        }),
        &catalog(),
    )
    .expect("the settings are read");

    assert_eq!(
        settings
            .get_provider(Some("local"))
            .expect("local exists")
            .context_windows()
            .get("qwen"),
        Some(&64000)
    );
    assert_eq!(
        settings.scoped_models,
        vec![
            ScopedModelConfig::new("local", "qwen"),
            ScopedModelConfig::new("local", "llama"),
        ]
    );
}

#[test]
fn settings_must_include_preferences_or_providers() {
    let error = provider_settings_from_json(
        &serde_json::json!({"default_provider": "openai"}),
        &catalog(),
    )
    .expect_err("there is nothing to read");

    assert_eq!(
        error.message(),
        "Provider settings must include provider_preferences or legacy providers"
    );
}

#[test]
fn invalid_headers_are_rejected() {
    let error = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen"],
                    "default_model": "qwen",
                    "headers": {"X-Test": 123},
                },
            ],
        }),
        &catalog(),
    )
    .expect_err("a numeric header value is not a header");

    assert_eq!(
        error.message(),
        "Provider field must be a string object: providers[local].headers"
    );
}

#[test]
fn an_invalid_timeout_is_rejected() {
    let error = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen"],
                    "default_model": "qwen",
                    "timeout_seconds": 0,
                },
            ],
        }),
        &catalog(),
    )
    .expect_err("a zero timeout is not usable");

    assert_eq!(
        error.message(),
        "Provider field must be greater than 0: providers[local].timeout_seconds"
    );
}

#[test]
fn invalid_retries_are_rejected() {
    let error = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen"],
                    "default_model": "qwen",
                    "max_retries": -1,
                },
            ],
        }),
        &catalog(),
    )
    .expect_err("a negative retry count is not usable");

    assert_eq!(
        error.message(),
        "Provider field must be 0 or greater: providers[local].max_retries"
    );

    let delay = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen"],
                    "default_model": "qwen",
                    "max_retry_delay_seconds": -0.5,
                },
            ],
        }),
        &catalog(),
    )
    .expect_err("a negative retry delay is not usable");

    assert_eq!(
        delay.message(),
        "Provider field must be 0 or greater: providers[local].max_retry_delay_seconds"
    );
}

#[test]
fn a_provider_config_rejects_invalid_numbers() {
    let timeout = OpenAiCompatibleProviderConfig {
        timeout_seconds: 0.0,
        ..OpenAiCompatibleProviderConfig::new("local")
    }
    .validate()
    .expect_err("a zero timeout is not usable");
    assert_eq!(
        timeout.message(),
        "Provider timeout_seconds must be greater than 0"
    );

    let retries = OpenAiCompatibleProviderConfig {
        max_retries: -1,
        ..OpenAiCompatibleProviderConfig::new("local")
    }
    .validate()
    .expect_err("a negative retry count is not usable");
    assert_eq!(
        retries.message(),
        "Provider max_retries must be 0 or greater"
    );

    let delay = OpenAiCompatibleProviderConfig {
        max_retry_delay_seconds: -1.0,
        ..OpenAiCompatibleProviderConfig::new("local")
    }
    .validate()
    .expect_err("a negative delay is not usable");
    assert_eq!(
        delay.message(),
        "Provider max_retry_delay_seconds must be 0 or greater"
    );
}

#[test]
fn a_model_metadata_key_outside_the_models_is_rejected() {
    let error = OpenAiCompatibleProviderConfig {
        models: vec!["qwen".to_string()],
        default_model: "qwen".to_string(),
        model_metadata: BTreeMap::from([("llama".to_string(), ProviderModelMetadata::default())]),
        ..OpenAiCompatibleProviderConfig::new("local")
    }
    .validate()
    .expect_err("metadata for an undeclared model is not usable");

    assert_eq!(
        error.message(),
        "Provider model_metadata key is not in models: llama"
    );
}

#[test]
fn resolve_provider_selection_uses_the_configured_defaults() {
    let settings = ProviderSettings {
        default_provider: "local".to_string(),
        providers: vec![openai_config("local")],
        ..ProviderSettings::default()
    };

    let selection =
        resolve_provider_selection(&settings, None, None).expect("the selection resolves");

    assert_eq!(selection.provider.name(), "local");
    assert_eq!(selection.model, DEFAULT_MODEL);
}

#[test]
fn an_unknown_provider_is_rejected() {
    let error = resolve_provider_selection(&ProviderSettings::default(), Some("missing"), None)
        .expect_err("the provider is not configured");

    assert_eq!(error.message(), "Unknown provider: missing");
}

#[test]
fn a_model_not_declared_for_the_provider_is_rejected() {
    let settings = ProviderSettings {
        default_provider: "local".to_string(),
        providers: vec![openai_config("local")],
        ..ProviderSettings::default()
    };

    let error = resolve_provider_selection(&settings, None, Some("llama"))
        .expect_err("the model is not declared");

    assert_eq!(
        error.message(),
        "Model is not configured for provider local: llama. Available models: gpt-5.4"
    );
}

#[test]
fn a_provider_without_a_default_model_is_rejected() {
    let settings = ProviderSettings {
        default_provider: "local".to_string(),
        providers: vec![ProviderConfig::OpenAiCompatible(
            OpenAiCompatibleProviderConfig {
                models: vec!["qwen".to_string()],
                default_model: String::new(),
                ..OpenAiCompatibleProviderConfig::new("local")
            },
        )],
        ..ProviderSettings::default()
    };

    let error = resolve_provider_selection(&settings, None, None)
        .expect_err("the provider declares no default model");

    assert_eq!(
        error.message(),
        "Provider local does not define a default model"
    );
}

#[test]
fn a_stored_credential_wins_over_the_environment() {
    let env = MapEnv::new().with("OPENROUTER_API_KEY", "env-key");
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://openrouter.ai/api/v1".to_string(),
        api_key_env: "OPENROUTER_API_KEY".to_string(),
        credential_name: Some("openrouter".to_string()),
        models: vec!["openai/gpt-4.1-mini".to_string()],
        default_model: "openai/gpt-4.1-mini".to_string(),
        ..OpenAiCompatibleProviderConfig::new("openrouter")
    });

    let key = api_key_from_provider(
        &provider,
        &env,
        Some(&StoredKey("openrouter", "stored-key")),
    )
    .expect("the key resolves");

    assert_eq!(key, "stored-key");
}

#[test]
fn the_environment_is_used_when_no_credential_is_stored() {
    let env = MapEnv::new().with("OPENROUTER_API_KEY", "env-key");
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "https://openrouter.ai/api/v1".to_string(),
        api_key_env: "OPENROUTER_API_KEY".to_string(),
        credential_name: Some("openrouter".to_string()),
        models: vec!["openai/gpt-4.1-mini".to_string()],
        default_model: "openai/gpt-4.1-mini".to_string(),
        ..OpenAiCompatibleProviderConfig::new("openrouter")
    });

    let key = api_key_from_provider(&provider, &env, Some(&NoCredentials))
        .expect("the key resolves from the environment");

    assert_eq!(key, "env-key");
}

#[test]
fn a_stored_oauth_access_token_is_used_when_no_api_key_is_stored() {
    struct StoredOAuth;

    impl CredentialReader for StoredOAuth {
        fn get(&self, _name: &str) -> Option<String> {
            None
        }

        fn get_oauth(&self, name: &str) -> Option<String> {
            (name == "anthropic").then(|| "oauth-access".to_string())
        }
    }

    let provider = ProviderConfig::Anthropic(AnthropicProviderConfig::new("anthropic"));

    let key = api_key_from_provider(&provider, &MapEnv::new(), Some(&StoredOAuth))
        .expect("the OAuth access token resolves");

    assert_eq!(key, "oauth-access");
    assert_eq!(provider.base_url(), "https://api.anthropic.com/v1");
}

#[test]
fn the_missing_key_message_names_the_variable_and_the_login_hint() {
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        api_key_env: "OPENROUTER_API_KEY".to_string(),
        credential_name: Some("openrouter".to_string()),
        ..OpenAiCompatibleProviderConfig::new("openrouter")
    });

    let error = api_key_from_provider(&provider, &MapEnv::new(), Some(&NoCredentials))
        .expect_err("no key is configured");

    assert_eq!(
        error.message(),
        "Missing provider API key. Set OPENROUTER_API_KEY or run /login openrouter."
    );

    let bare = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        api_key_env: "LOCAL_API_KEY".to_string(),
        ..OpenAiCompatibleProviderConfig::new("local")
    });

    let error =
        api_key_from_provider(&bare, &MapEnv::new(), None).expect_err("no key is configured");

    assert_eq!(
        error.message(),
        "Missing provider API key. Set LOCAL_API_KEY."
    );
}

#[test]
fn usable_credentials_check_the_stored_key_and_the_environment() {
    let mut env = MapEnv::new();
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        api_key_env: "OPENROUTER_API_KEY".to_string(),
        credential_name: Some("openrouter".to_string()),
        ..OpenAiCompatibleProviderConfig::new("openrouter")
    });

    assert!(!provider_has_usable_credentials(
        &provider,
        &env,
        Some(&NoCredentials)
    ));
    assert!(provider_has_usable_credentials(
        &provider,
        &env,
        Some(&StoredKey("openrouter", "stored-key"))
    ));

    env = env.with("OPENROUTER_API_KEY", "env-key");

    assert!(provider_has_usable_credentials(
        &provider,
        &env,
        Some(&NoCredentials)
    ));
}

#[test]
fn the_openai_base_url_environment_value_overrides_the_configured_url() {
    let env = MapEnv::new().with("OPENAI_BASE_URL", "https://proxy.example/v1/");
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        name: "openai".to_string(),
        credential_name: Some("openai".to_string()),
        ..OpenAiCompatibleProviderConfig::new("openai")
    });

    assert_eq!(
        api_key_from_provider(&provider, &env, Some(&StoredKey("openai", "stored-key")))
            .expect("the key resolves"),
        "stored-key"
    );
    assert_eq!(
        openai_compatible_base_url(&provider, &env, None),
        "https://proxy.example/v1"
    );
}

#[test]
fn a_configured_trailing_slash_is_stripped() {
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "http://localhost:11434/v1/".to_string(),
        api_key_env: "LOCAL_API_KEY".to_string(),
        models: vec!["qwen".to_string()],
        default_model: "qwen".to_string(),
        ..OpenAiCompatibleProviderConfig::new("local")
    });

    assert_eq!(
        openai_compatible_base_url(&provider, &MapEnv::new(), None),
        "http://localhost:11434/v1"
    );
}

#[test]
fn a_non_openai_provider_ignores_the_openai_base_url_variable() {
    let env = MapEnv::new().with("OPENAI_BASE_URL", "https://proxy.example/v1");
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        base_url: "http://localhost:11434/v1".to_string(),
        api_key_env: "LOCAL_API_KEY".to_string(),
        ..OpenAiCompatibleProviderConfig::new("local")
    });

    assert_eq!(
        openai_compatible_base_url(&provider, &env, None),
        "http://localhost:11434/v1"
    );
}

#[test]
fn model_metadata_drives_capabilities_and_overrides() {
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        api_key_env: "CUSTOM_API_KEY".to_string(),
        models: vec!["text".to_string(), "vision".to_string()],
        default_model: "text".to_string(),
        model_metadata: BTreeMap::from([
            (
                "text".to_string(),
                ProviderModelMetadata {
                    input: vec!["text".to_string()],
                    max_tokens: Some(4096),
                    headers: BTreeMap::from([("X-Model".to_string(), "text".to_string())]),
                    ..ProviderModelMetadata::default()
                },
            ),
            (
                "vision".to_string(),
                ProviderModelMetadata {
                    input: vec!["text".to_string(), "image".to_string()],
                    ..ProviderModelMetadata::default()
                },
            ),
        ]),
        ..OpenAiCompatibleProviderConfig::new("custom")
    });

    assert!(!provider_model_supports_images(&provider, Some("text")));
    assert!(provider_model_supports_images(&provider, Some("vision")));
    assert_eq!(
        provider_model_max_tokens(&provider, Some("text")),
        Some(4096)
    );
    assert_eq!(provider_model_max_tokens(&provider, Some("vision")), None);
}

#[test]
fn provider_thinking_levels_follow_the_model_metadata_map() {
    let provider = ProviderConfig::OpenAiCompatible(OpenAiCompatibleProviderConfig {
        models: vec!["k3".to_string(), "kimi-for-coding".to_string()],
        default_model: "k3".to_string(),
        thinking_levels: Some(vec![
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::Xhigh,
        ]),
        thinking_default: Some(ThinkingLevel::Xhigh),
        thinking_parameter: Some(tau_coding::provider_config::ThinkingParameter::ReasoningEffort),
        model_metadata: BTreeMap::from([
            (
                "k3".to_string(),
                ProviderModelMetadata {
                    reasoning: Some(true),
                    thinking_level_map: BTreeMap::from([
                        (ThinkingLevel::Off, None),
                        (ThinkingLevel::Minimal, None),
                        (ThinkingLevel::Low, Some("low".to_string())),
                        (ThinkingLevel::Medium, None),
                        (ThinkingLevel::High, Some("high".to_string())),
                        (ThinkingLevel::Xhigh, Some("max".to_string())),
                    ]),
                    ..ProviderModelMetadata::default()
                },
            ),
            (
                "kimi-for-coding".to_string(),
                ProviderModelMetadata {
                    reasoning: Some(false),
                    ..ProviderModelMetadata::default()
                },
            ),
        ]),
        ..OpenAiCompatibleProviderConfig::new("kimi-code")
    });

    assert_eq!(
        provider_thinking_levels(&provider, Some("k3")),
        vec![
            ThinkingLevel::Low,
            ThinkingLevel::High,
            ThinkingLevel::Xhigh
        ]
    );
    assert_eq!(
        provider_thinking_levels(&provider, Some("kimi-for-coding")),
        Vec::<ThinkingLevel>::new(),
        "a model that is not a reasoning model has no levels"
    );
    assert_eq!(
        provider_default_thinking_level(&provider, Some("k3")),
        Some(ThinkingLevel::Xhigh)
    );
}

#[test]
fn huggingface_inference_provider_values_are_validated() {
    assert_eq!(
        validate_huggingface_inference_provider(" deepinfra ").expect("a valid route"),
        "deepinfra"
    );

    let policy = validate_huggingface_inference_provider("fastest")
        .expect_err("a routing policy is not an explicit provider");
    assert_eq!(
        policy.message(),
        "Hugging Face inference provider must be explicit, not routing policy: fastest"
    );

    let shape = validate_huggingface_inference_provider("bad route!")
        .expect_err("a route with a space is not a provider id");
    assert_eq!(
        shape.message(),
        "Hugging Face inference provider must contain only letters, numbers, '.', '_', or '-'"
    );
}

#[test]
fn inference_providers_are_rejected_outside_huggingface() {
    let error = OpenAiCompatibleProviderConfig {
        models: vec!["qwen".to_string()],
        default_model: "qwen".to_string(),
        inference_providers: BTreeMap::from([("qwen".to_string(), "deepinfra".to_string())]),
        ..OpenAiCompatibleProviderConfig::new("local")
    }
    .validate()
    .expect_err("only huggingface routes by inference provider");

    assert_eq!(
        error.message(),
        "inference_providers preferences are only supported for the huggingface provider"
    );
}

#[test]
fn provider_settings_to_json_carries_only_preferences() {
    let settings = ProviderSettings {
        default_provider: "local".to_string(),
        providers: vec![ProviderConfig::OpenAiCompatible(
            OpenAiCompatibleProviderConfig {
                base_url: "http://localhost:11434/v1".to_string(),
                api_key_env: "LOCAL_API_KEY".to_string(),
                models: vec!["qwen".to_string()],
                default_model: "qwen".to_string(),
                headers: BTreeMap::from([("X-Test".to_string(), "yes".to_string())]),
                timeout_seconds: 120.0,
                max_retries: 2,
                max_retry_delay_seconds: 0.5,
                inference_providers: BTreeMap::from([(
                    "qwen".to_string(),
                    "deepinfra".to_string(),
                )]),
                ..OpenAiCompatibleProviderConfig::new("local")
            },
        )],
        scoped_models: vec![ScopedModelConfig::new("local", "qwen")],
    };

    let json = settings.to_json();

    assert_eq!(json["schema_version"], serde_json::json!(2));
    assert_eq!(json["default_provider"], serde_json::json!("local"));
    assert_eq!(
        json["provider_preferences"]["local"]["default_model"],
        serde_json::json!("qwen")
    );
    assert_eq!(
        json["provider_preferences"]["local"]["timeout_seconds"],
        serde_json::json!(120.0)
    );
    assert_eq!(
        json["scoped_models"],
        serde_json::json!([{"provider": "local", "model": "qwen"}])
    );
    assert!(
        json["provider_preferences"]["local"]
            .get("base_url")
            .is_none(),
        "a preference never rewrites the provider definition"
    );
}

#[test]
fn provider_settings_from_json_reads_back_its_own_preferences() {
    let settings = ProviderSettings {
        default_provider: "openai".to_string(),
        providers: vec![ProviderConfig::OpenAiCompatible(
            OpenAiCompatibleProviderConfig {
                models: vec!["gpt-5.4".to_string(), "gpt-5-mini".to_string()],
                default_model: "gpt-5-mini".to_string(),
                headers: BTreeMap::from([("X-Test".to_string(), "yes".to_string())]),
                timeout_seconds: 120.0,
                max_retries: 2,
                max_retry_delay_seconds: 0.5,
                ..OpenAiCompatibleProviderConfig::new("openai")
            },
        )],
        scoped_models: vec![ScopedModelConfig::new("openai", "gpt-5-mini")],
    };

    let json = settings.to_json();
    let reloaded = provider_settings_from_json(&json, &catalog()).expect("the preferences parse");
    let provider = reloaded
        .get_provider(Some("openai"))
        .expect("openai exists");

    assert_eq!(provider.default_model(), "gpt-5-mini");
    assert_eq!(provider.timeout_seconds(), 120.0);
    assert_eq!(provider.max_retries(), 2);
    assert_eq!(reloaded.scoped_models, settings.scoped_models);
}

#[test]
fn an_unknown_thinking_mode_is_rejected() {
    let error = provider_settings_from_json(
        &serde_json::json!({
            "default_provider": "local",
            "providers": [
                {
                    "type": "openai-compatible",
                    "name": "local",
                    "base_url": "http://localhost:11434/v1",
                    "api_key_env": "LOCAL_API_KEY",
                    "models": ["qwen"],
                    "default_model": "qwen",
                    "thinking_default": "very-high",
                },
            ],
        }),
        &catalog(),
    )
    .expect_err("an unknown mode is not readable");

    assert_eq!(
        error.message(),
        "Unknown thinking mode: very-high. Available modes: \
         off, minimal, low, medium, high, xhigh, max"
    );
}
