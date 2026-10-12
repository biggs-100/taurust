//! The adapter-level provider lifecycle events.
//!
//! These are the events a provider adapter emits around a request, distinct from
//! the canonical assistant stream in `tau-protocol`. Ported from
//! tau's `tau_ai/_provider_events.py`.

use serde_json::json;
use tau_ai::provider_events::ProviderEvent;
use tau_protocol::content::ToolCall;
use tau_protocol::message::AssistantMessage;

fn message() -> AssistantMessage {
    AssistantMessage::default()
}

#[test]
fn the_adapter_event_type_tags_match_pi() {
    let cases: Vec<(ProviderEvent, &str)> = vec![
        (
            ProviderEvent::ResponseStart {
                model: "claude".into(),
                response_provider: None,
            },
            "response_start",
        ),
        (
            ProviderEvent::Retry {
                attempt: 2,
                max_attempts: 3,
                delay_seconds: 0.5,
                message: "retrying".into(),
                data: None,
            },
            "retry",
        ),
        (
            ProviderEvent::TextDelta { delta: "hi".into() },
            "text_delta",
        ),
        (
            ProviderEvent::ThinkingDelta {
                delta: "hmm".into(),
            },
            "thinking_delta",
        ),
        (
            ProviderEvent::ToolCall {
                tool_call: ToolCall {
                    id: "call-1".into(),
                    name: "read".into(),
                    arguments: Default::default(),
                    thought_signature: None,
                },
            },
            "tool_call",
        ),
        (
            ProviderEvent::ResponseEnd {
                message: Box::new(message()),
                finish_reason: Some("stop".into()),
            },
            "response_end",
        ),
        (
            ProviderEvent::Error {
                message: "boom".into(),
                data: None,
                response_provider: None,
            },
            "error",
        ),
    ];

    for (event, expected) in cases {
        assert_eq!(
            serde_json::to_value(&event).unwrap()["type"],
            json!(expected)
        );
        assert_eq!(event.kind(), expected);
    }
}

#[test]
fn the_adapter_event_fields_use_camel_case() {
    let event = ProviderEvent::ResponseEnd {
        message: Box::new(message()),
        finish_reason: Some("stop".into()),
    };

    assert_eq!(
        serde_json::to_value(&event).unwrap()["finishReason"],
        json!("stop")
    );

    let start = ProviderEvent::ResponseStart {
        model: "claude".into(),
        response_provider: Some("bedrock".into()),
    };
    assert_eq!(
        serde_json::to_value(&start).unwrap()["responseProvider"],
        json!("bedrock")
    );
}
