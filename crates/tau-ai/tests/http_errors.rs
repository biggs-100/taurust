//! The provider HTTP error taxonomy.

use tau_ai::http_errors::{
    provider_error_detail_from_mapping, provider_http_error_detail, provider_http_error_message,
};
use tau_protocol::JsonValue;

fn object(value: JsonValue) -> serde_json::Map<String, JsonValue> {
    value.as_object().expect("a JSON object").clone()
}

#[test]
fn the_message_names_the_provider_the_status_and_the_model() {
    let message = provider_http_error_message(
        "Anthropic",
        429,
        "{\"error\":{\"message\":\"rate limited\"}}",
        Some("claude-sonnet"),
    );

    assert_eq!(
        message,
        "Anthropic request failed with status 429 for model claude-sonnet: rate limited"
    );
}

#[test]
fn the_message_omits_the_model_when_it_is_unknown() {
    let message = provider_http_error_message("Anthropic", 401, "   ", None);

    assert_eq!(message, "Anthropic request failed with status 401");
}

#[test]
fn a_json_error_body_yields_its_message() {
    assert_eq!(
        provider_http_error_detail("{\"error\":{\"message\":\"rate limited\"}}"),
        "rate limited"
    );
}

#[test]
fn a_json_error_body_falls_back_to_its_code() {
    assert_eq!(
        provider_http_error_detail("{\"error\":{\"code\":\"overloaded_error\"}}"),
        "overloaded_error"
    );
}

#[test]
fn a_top_level_message_or_detail_is_used_when_there_is_no_error_object() {
    assert_eq!(provider_http_error_detail("{\"message\":\"boom\"}"), "boom");
    assert_eq!(provider_http_error_detail("{\"detail\":\"nope\"}"), "nope");
}

#[test]
fn a_nested_error_object_is_searched_recursively() {
    let value = object(serde_json::json!({ "error": { "detail": "deep" } }));

    assert_eq!(provider_error_detail_from_mapping(&value), "deep");
}

#[test]
fn a_non_json_body_is_trimmed_and_capped() {
    let body = "  service unavailable  ";

    assert_eq!(provider_http_error_detail(body), "service unavailable");

    let long = "x".repeat(1500);
    assert_eq!(provider_http_error_detail(&long).len(), 1000);
}

#[test]
fn a_blank_body_leaves_only_the_prefix() {
    assert_eq!(provider_http_error_detail("   "), "");
    assert_eq!(
        provider_http_error_message("Anthropic", 500, "   ", None),
        "Anthropic request failed with status 500"
    );
}

#[test]
fn a_body_with_no_extractable_detail_is_passed_through_verbatim() {
    // The reference falls back to the trimmed body whenever the extracted detail
    // is empty, so an empty JSON object becomes the detail itself. Ugly, but it
    // is what tau does, and the port matches the reference rather than the
    // prettier alternative.
    assert_eq!(provider_http_error_detail("{}"), "{}");
    assert_eq!(
        provider_http_error_message("Anthropic", 500, "{}", None),
        "Anthropic request failed with status 500: {}"
    );
}
