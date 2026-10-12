//! Pi-compatible wire format for the shared vocabulary.
//!
//! The assertions mirror tau's own `tests/test_agent_types.py`, so the Rust
//! port is checked against the same contract the Python original is.

use serde_json::json;
use tau_protocol::content::{TextContent, ThinkingContent, ToolCall};
use tau_protocol::message::{
    AgentMessage, AssistantMessage, ResponseTiming, StopReason, ToolResultMessage, Usage,
    UserMessage,
};

#[test]
fn user_message_serializes_with_the_pi_wire_shape() {
    let message = UserMessage {
        content: "hello".into(),
        timestamp: 123,
        ..UserMessage::default()
    };

    assert_eq!(
        serde_json::to_value(&message).unwrap(),
        json!({ "role": "user", "content": "hello", "timestamp": 123 })
    );
}

#[test]
fn assistant_message_keeps_ordered_content_blocks() {
    let message = AssistantMessage {
        content: vec![
            TextContent {
                text: "I'll read that.".into(),
                text_signature: None,
            }
            .into(),
            ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: json!({ "path": "README.md" }).as_object().unwrap().clone(),
                thought_signature: None,
            }
            .into(),
        ],
        model: "fake".into(),
        timestamp: 123,
        ..AssistantMessage::default()
    };

    assert_eq!(message.text(), "I'll read that.");
    let payload = serde_json::to_value(&message).unwrap();
    assert_eq!(
        payload["content"][1],
        json!({
            "type": "toolCall",
            "id": "call-1",
            "name": "read",
            "arguments": { "path": "README.md" },
            "thoughtSignature": null,
        })
    );
}

#[test]
fn thinking_blocks_keep_their_signatures() {
    let message = AssistantMessage {
        content: vec![
            ThinkingContent {
                thinking: "plan".into(),
                thinking_signature: Some("reasoning_content".into()),
                redacted: false,
            }
            .into(),
            TextContent {
                text: "done".into(),
                text_signature: None,
            }
            .into(),
        ],
        model: "fake".into(),
        timestamp: 123,
        ..AssistantMessage::default()
    };

    assert_eq!(message.thinking_text(), "plan");
    assert_eq!(
        serde_json::to_value(&message).unwrap()["content"][0],
        json!({
            "type": "thinking",
            "thinking": "plan",
            "thinkingSignature": "reasoning_content",
            "redacted": false,
        })
    );
}

#[test]
fn assistant_message_serializes_response_timing() {
    let message = AssistantMessage {
        content: vec![
            TextContent {
                text: "done".into(),
                text_signature: None,
            }
            .into(),
        ],
        timing: Some(ResponseTiming {
            time_to_first_output_ms: Some(1200),
            total_duration_ms: 5000,
        }),
        timestamp: 123,
        ..AssistantMessage::default()
    };

    assert_eq!(
        serde_json::to_value(&message).unwrap()["timing"],
        json!({ "timeToFirstOutputMs": 1200, "totalDurationMs": 5000 })
    );
}

#[test]
fn tool_result_message_records_canonical_tool_output() {
    let message = ToolResultMessage {
        tool_call_id: "call-1".into(),
        tool_name: "read".into(),
        content: vec![
            TextContent {
                text: "file contents".into(),
                text_signature: None,
            }
            .into(),
        ],
        details: Some(json!({ "bytes": 13 })),
        is_error: false,
        timestamp: 123,
        ..ToolResultMessage::default()
    };

    assert_eq!(message.text(), "file contents");
    assert_eq!(message.details, Some(json!({ "bytes": 13 })));
    assert_eq!(
        serde_json::to_value(&message).unwrap()["toolCallId"],
        json!("call-1")
    );
}

#[test]
fn a_tool_result_accepts_pi_camel_case_wire_keys() {
    let message: ToolResultMessage = serde_json::from_value(json!({
        "role": "toolResult",
        "toolCallId": "call-1",
        "toolName": "read",
        "content": [{ "type": "text", "text": "file contents" }],
        "timestamp": 123,
        "isError": false,
    }))
    .unwrap();

    assert_eq!(message.tool_call_id, "call-1");
    assert_eq!(message.tool_name, "read");
    assert!(!message.is_error);
    assert_eq!(message.text(), "file contents");
}

#[test]
fn usage_totals_are_field_wise_and_keep_optional_fields_absent() {
    let total = Usage::total([first_usage(), second_usage()]);

    assert_eq!(total.input, 3);
    assert_eq!(total.output, 7);
    assert_eq!(total.total_tokens, 10);
    assert_eq!(total.cache_write_1h, Some(9));
    assert_eq!(total.reasoning, None);
    assert_eq!(total.cost.total, 0.5);
}

#[test]
fn messages_discriminate_on_the_role_tag() {
    let cases: Vec<(AgentMessage, &str)> = vec![
        (
            UserMessage {
                content: "hi".into(),
                timestamp: 1,
                ..UserMessage::default()
            }
            .into(),
            "user",
        ),
        (AssistantMessage::default().into(), "assistant"),
        (ToolResultMessage::default().into(), "toolResult"),
    ];

    for (message, role) in cases {
        assert_eq!(serde_json::to_value(&message).unwrap()["role"], json!(role));
    }
}

#[test]
fn unknown_fields_are_rejected() {
    let result = serde_json::from_value::<UserMessage>(json!({
        "role": "user",
        "content": "hello",
        "unexpected": true,
    }));

    assert!(result.is_err(), "an unknown field must not deserialize");
}

#[test]
fn the_stop_reason_uses_the_pi_wire_spelling() {
    assert_eq!(
        serde_json::to_value(StopReason::ToolUse).unwrap(),
        json!("toolUse")
    );
}

fn first_usage() -> Usage {
    Usage {
        input: 1,
        output: 2,
        total_tokens: 3,
        cost: tau_protocol::message::UsageCost {
            total: 0.25,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn second_usage() -> Usage {
    Usage {
        input: 2,
        output: 5,
        cache_write_1h: Some(9),
        total_tokens: 7,
        cost: tau_protocol::message::UsageCost {
            total: 0.25,
            ..Default::default()
        },
        ..Default::default()
    }
}
