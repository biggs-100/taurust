//! Provider configuration read from the environment.

use tau_ai::env::{
    DEFAULT_ANTHROPIC_BASE_URL, DEFAULT_OPENAI_COMPATIBLE_BASE_URL,
    DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES, DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS,
    DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS, EnvSource, MapEnv, non_negative_float_from_env,
    non_negative_int_from_env, required_env, timeout_seconds_from_env,
};

fn env(name: &str, value: &str) -> MapEnv {
    MapEnv::new().with(name, value)
}

fn message<T: std::fmt::Debug>(result: Result<T, tau_ai::env::ConfigError>) -> String {
    result
        .expect_err("expected a configuration error")
        .to_string()
}

#[test]
fn the_defaults_match_tau() {
    assert_eq!(
        DEFAULT_OPENAI_COMPATIBLE_BASE_URL,
        "https://api.openai.com/v1"
    );
    assert_eq!(DEFAULT_ANTHROPIC_BASE_URL, "https://api.anthropic.com/v1");
    assert_eq!(DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS, 60.0);
    assert_eq!(DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES, 2);
    assert_eq!(DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS, 1.0);
}

#[test]
fn an_unset_variable_yields_the_default() {
    let empty = MapEnv::new();

    assert_eq!(
        timeout_seconds_from_env(&empty, "OPENAI_TIMEOUT_SECONDS", 60.0).unwrap(),
        60.0
    );
    assert_eq!(
        non_negative_int_from_env(&empty, "OPENAI_MAX_RETRIES", 2).unwrap(),
        2
    );
    assert_eq!(
        non_negative_float_from_env(&empty, "OPENAI_MAX_RETRY_DELAY_SECONDS", 1.0).unwrap(),
        1.0
    );
}

#[test]
fn a_present_variable_overrides_the_default() {
    assert_eq!(
        timeout_seconds_from_env(
            &env("OPENAI_TIMEOUT_SECONDS", "5.5"),
            "OPENAI_TIMEOUT_SECONDS",
            60.0
        )
        .unwrap(),
        5.5
    );
    assert_eq!(
        non_negative_int_from_env(&env("OPENAI_MAX_RETRIES", "0"), "OPENAI_MAX_RETRIES", 2)
            .unwrap(),
        0
    );
}

#[test]
fn a_missing_api_key_is_reported_by_name() {
    assert_eq!(
        message(required_env(&MapEnv::new(), "OPENAI_API_KEY")),
        "Missing required environment variable: OPENAI_API_KEY"
    );
    assert_eq!(
        message(required_env(&env("OPENAI_API_KEY", ""), "OPENAI_API_KEY")),
        "Missing required environment variable: OPENAI_API_KEY"
    );
    assert_eq!(
        required_env(&env("OPENAI_API_KEY", "sk-1"), "OPENAI_API_KEY").unwrap(),
        "sk-1"
    );
}

#[test]
fn a_non_numeric_timeout_is_rejected() {
    assert_eq!(
        message(timeout_seconds_from_env(
            &env("OPENAI_TIMEOUT_SECONDS", "abc"),
            "OPENAI_TIMEOUT_SECONDS",
            60.0
        )),
        "Environment variable must be a number: OPENAI_TIMEOUT_SECONDS"
    );
}

#[test]
fn a_non_positive_timeout_is_rejected() {
    assert_eq!(
        message(timeout_seconds_from_env(
            &env("OPENAI_TIMEOUT_SECONDS", "0"),
            "OPENAI_TIMEOUT_SECONDS",
            60.0
        )),
        "Environment variable must be greater than 0: OPENAI_TIMEOUT_SECONDS"
    );
}

#[test]
fn a_non_integer_retry_count_is_rejected() {
    assert_eq!(
        message(non_negative_int_from_env(
            &env("OPENAI_MAX_RETRIES", "two"),
            "OPENAI_MAX_RETRIES",
            2
        )),
        "Environment variable must be an integer: OPENAI_MAX_RETRIES"
    );
}

#[test]
fn a_negative_retry_count_is_rejected() {
    assert_eq!(
        message(non_negative_int_from_env(
            &env("OPENAI_MAX_RETRIES", "-1"),
            "OPENAI_MAX_RETRIES",
            2
        )),
        "Environment variable must be 0 or greater: OPENAI_MAX_RETRIES"
    );
}

#[test]
fn a_negative_retry_delay_is_rejected() {
    assert_eq!(
        message(non_negative_float_from_env(
            &env("OPENAI_MAX_RETRY_DELAY_SECONDS", "-0.5"),
            "OPENAI_MAX_RETRY_DELAY_SECONDS",
            1.0
        )),
        "Environment variable must be 0 or greater: OPENAI_MAX_RETRY_DELAY_SECONDS"
    );
}

#[test]
fn the_process_environment_is_available_as_a_source() {
    let source = tau_ai::env::ProcessEnv;
    let missing = format!("TAURUST_DEFINITELY_UNSET_{}", std::process::id());

    assert_eq!(source.get(&missing), None);
}
