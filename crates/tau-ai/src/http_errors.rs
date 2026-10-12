//! Turning a provider HTTP failure into a short, secret-free message.
//!
//! Ported from tau's `tau_ai/http_errors.py`. The detail is searched for in the
//! usual provider error shapes before falling back to the raw body, so a user
//! sees `rate limited` rather than a page of JSON.

use tau_protocol::{JsonObject, JsonValue};

/// The longest raw body kept when nothing structured can be extracted.
const MAX_ERROR_DETAIL_LENGTH: usize = 1000;

/// Build an actionable message for a failed provider request.
pub fn provider_http_error_message(
    provider_name: &str,
    status_code: u16,
    body: &str,
    model: Option<&str>,
) -> String {
    let mut prefix = format!("{provider_name} request failed with status {status_code}");
    if let Some(model) = model {
        prefix = format!("{prefix} for model {model}");
    }

    let detail = provider_http_error_detail(body);
    if detail.is_empty() {
        prefix
    } else {
        format!("{prefix}: {detail}")
    }
}

/// Extract the most useful detail from a provider error body.
///
/// A structured error object wins; otherwise the trimmed body is used, capped so
/// a stray HTML page cannot flood a terminal.
pub fn provider_http_error_detail(body: &str) -> String {
    if let Some(object) = serde_json::from_str::<JsonValue>(body)
        .ok()
        .and_then(|value| value.as_object().cloned())
    {
        let detail = provider_error_detail_from_mapping(&object);
        if !detail.is_empty() {
            return detail;
        }
    }

    body.trim().chars().take(MAX_ERROR_DETAIL_LENGTH).collect()
}

/// Return the most useful message or code from a provider error object.
pub fn provider_error_detail_from_mapping(value: &JsonObject) -> String {
    if let Some(error) = value.get("error").and_then(JsonValue::as_object) {
        if let Some(message) = non_empty_str(error.get("message")) {
            return message.to_string();
        }
        if let Some(code) = non_empty_str(error.get("code")) {
            return code.to_string();
        }
    }

    for key in ["message", "detail", "error"] {
        match value.get(key) {
            Some(JsonValue::String(text)) if !text.is_empty() => return text.clone(),
            Some(JsonValue::Object(nested)) => {
                let nested = provider_error_detail_from_mapping(nested);
                if !nested.is_empty() {
                    return nested;
                }
            }
            _ => {}
        }
    }

    String::new()
}

fn non_empty_str(value: Option<&JsonValue>) -> Option<&str> {
    value
        .and_then(JsonValue::as_str)
        .filter(|text| !text.is_empty())
}
