//! Pi-compatible streaming events.
//!
//! The type tags mirror tau's `tests/test_pi_event_protocol.py`, which asserts the
//! stable Pi event names for both event families.

use serde_json::json;
use tau_protocol::agent_event::AgentEvent;
use tau_protocol::content::{TextContent, ToolCall};
use tau_protocol::message::{AgentMessage, AssistantMessage, StopReason, ToolResultMessage};
use tau_protocol::provider_event::{AssistantMessageEvent, DoneReason, ErrorReason};
use tau_protocol::tool::AgentToolResult;

fn empty() -> AssistantMessage {
    AssistantMessage {
        model: "fake".into(),
        ..AssistantMessage::default()
    }
}

fn hello() -> AssistantMessage {
    AssistantMessage {
        model: "fake".into(),
        content: vec![
            TextContent {
                text: "hello".into(),
                text_signature: None,
            }
            .into(),
        ],
        ..AssistantMessage::default()
    }
}

#[test]
fn the_assistant_event_type_tags_match_pi() {
    let partial = empty();
    let final_message = hello();

    let cases: Vec<(AssistantMessageEvent, &str)> = vec![
        (
            AssistantMessageEvent::Start {
                partial: partial.clone(),
            },
            "start",
        ),
        (
            AssistantMessageEvent::TextStart {
                content_index: 0,
                partial: partial.clone(),
            },
            "text_start",
        ),
        (
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hello".into(),
                partial: final_message.clone(),
            },
            "text_delta",
        ),
        (
            AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "hello".into(),
                partial: final_message.clone(),
            },
            "text_end",
        ),
        (
            AssistantMessageEvent::ThinkingStart {
                content_index: 0,
                partial: partial.clone(),
            },
            "thinking_start",
        ),
        (
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "plan".into(),
                partial: partial.clone(),
            },
            "thinking_delta",
        ),
        (
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content: "plan".into(),
                partial: partial.clone(),
            },
            "thinking_end",
        ),
        (
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                partial: partial.clone(),
            },
            "toolcall_start",
        ),
        (
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{".into(),
                partial: partial.clone(),
            },
            "toolcall_delta",
        ),
        (
            AssistantMessageEvent::ToolCallEnd {
                content_index: 0,
                tool_call: ToolCall {
                    id: "call-1".into(),
                    name: "read".into(),
                    arguments: Default::default(),
                    thought_signature: None,
                },
                partial: partial.clone(),
            },
            "toolcall_end",
        ),
        (
            AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: final_message.clone(),
            },
            "done",
        ),
        (
            AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: AssistantMessage::default(),
            },
            "error",
        ),
    ];

    for (event, expected) in cases {
        assert_eq!(
            serde_json::to_value(&event).unwrap()["type"],
            json!(expected),
            "the wire type tag must match Pi"
        );
        assert_eq!(event.kind(), expected);
    }
}

#[test]
fn the_agent_event_type_tags_match_pi() {
    let result = AgentToolResult::from("contents");

    let cases: Vec<(AgentEvent, &str)> = vec![
        (AgentEvent::AgentStart, "agent_start"),
        (AgentEvent::AgentEnd { messages: vec![] }, "agent_end"),
        (AgentEvent::TurnStart, "turn_start"),
        (
            AgentEvent::TurnEnd {
                message: empty().into(),
                tool_results: vec![],
            },
            "turn_end",
        ),
        (
            AgentEvent::MessageStart {
                message: empty().into(),
            },
            "message_start",
        ),
        (
            AgentEvent::MessageUpdate {
                message: empty().into(),
                assistant_message_event: Box::new(AssistantMessageEvent::Start {
                    partial: empty(),
                }),
            },
            "message_update",
        ),
        (
            AgentEvent::MessageEnd {
                message: hello().into(),
            },
            "message_end",
        ),
        (
            AgentEvent::ToolExecutionStart {
                tool_call_id: "call-1".into(),
                tool_name: "read".into(),
                args: json!({ "path": "README.md" }).as_object().unwrap().clone(),
            },
            "tool_execution_start",
        ),
        (
            AgentEvent::ToolExecutionUpdate {
                tool_call_id: "call-1".into(),
                tool_name: "read".into(),
                args: Default::default(),
                partial_result: result.clone(),
            },
            "tool_execution_update",
        ),
        (
            AgentEvent::ToolExecutionEnd {
                tool_call_id: "call-1".into(),
                tool_name: "read".into(),
                result,
                is_error: false,
            },
            "tool_execution_end",
        ),
    ];

    for (event, expected) in cases {
        assert_eq!(
            serde_json::to_value(&event).unwrap()["type"],
            json!(expected),
            "the wire type tag must match Pi"
        );
        assert_eq!(event.kind(), expected);
    }
}

#[test]
fn the_message_update_event_uses_the_pi_camel_case_field() {
    let event = AgentEvent::MessageUpdate {
        message: empty().into(),
        assistant_message_event: Box::new(AssistantMessageEvent::Start { partial: empty() }),
    };

    let payload = serde_json::to_value(&event).unwrap();
    assert_eq!(payload["assistantMessageEvent"]["type"], json!("start"));
}

#[test]
fn the_tool_execution_fields_use_camel_case() {
    let event = AgentEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "read".into(),
        result: AgentToolResult::from("contents"),
        is_error: true,
    };

    let payload = serde_json::to_value(&event).unwrap();
    assert_eq!(payload["toolCallId"], json!("call-1"));
    assert_eq!(payload["toolName"], json!("read"));
    assert_eq!(payload["isError"], json!(true));
}

#[test]
fn an_agent_end_event_round_trips_its_messages() {
    let messages: Vec<AgentMessage> = vec![
        hello().into(),
        ToolResultMessage {
            tool_call_id: "call-1".into(),
            tool_name: "read".into(),
            content: vec![
                TextContent {
                    text: "contents".into(),
                    text_signature: None,
                }
                .into(),
            ],
            ..ToolResultMessage::default()
        }
        .into(),
    ];

    let event = AgentEvent::AgentEnd {
        messages: messages.clone(),
    };
    let encoded = serde_json::to_string(&event).unwrap();
    let decoded: AgentEvent = serde_json::from_str(&encoded).unwrap();

    assert_eq!(decoded, event);
    assert_eq!(messages[0].role(), "assistant");
    assert_eq!(messages[1].role(), "toolResult");
}

#[test]
fn the_tool_result_terminate_flag_is_optional_and_absent_by_default() {
    let result = AgentToolResult::from("done");

    assert_eq!(result.terminate, None);
    assert_eq!(result.text(), "done");
    assert_eq!(
        serde_json::to_value(&result).unwrap()["terminate"],
        json!(null)
    );
}

#[test]
fn a_finished_response_keeps_its_stop_reason() {
    let message = AssistantMessage {
        stop_reason: StopReason::ToolUse,
        ..hello()
    };
    let event = AssistantMessageEvent::Done {
        reason: DoneReason::ToolUse,
        message,
    };

    let payload = serde_json::to_value(&event).unwrap();
    assert_eq!(payload["reason"], json!("toolUse"));
    assert_eq!(payload["message"]["stopReason"], json!("toolUse"));
}

#[test]
fn events_tolerate_unknown_fields_while_messages_reject_them() {
    // Serde silently ignores `deny_unknown_fields` on an internally tagged enum,
    // so an event cannot be made strict. This was verified rather than assumed:
    // the strict version of this test failed with the attribute in place.
    //
    // Leniency is the right call here: a transient wire event must not break the
    // stream when a provider adds a field. A stored message must stay strict, and
    // does, because a session file with a typo should be caught.
    let event = serde_json::from_value::<AgentEvent>(json!({
        "type": "agent_start",
        "unexpected": true,
    }));
    assert!(event.is_ok(), "an event tolerates an unknown field");

    let message = serde_json::from_value::<tau_protocol::message::UserMessage>(json!({
        "role": "user",
        "content": "hello",
        "unexpected": true,
    }));
    assert!(
        message.is_err(),
        "a stored message rejects an unknown field"
    );
}
