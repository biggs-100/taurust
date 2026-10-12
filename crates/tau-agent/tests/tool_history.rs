//! Provider-safe repair of a transcript whose tool calls and results do not line up.
//!
//! Mirrors tau's `tests/test_tool_history.py`, which is the reference's own
//! contract for this function.

use serde_json::json;
use tau_agent::tool_history::{INTERRUPTED_TOOL_RESULT, repair_tool_history};
use tau_protocol::content::{AssistantContent, TextContent, ToolCall};
use tau_protocol::message::{AgentMessage, AssistantMessage, ToolResultMessage, UserMessage};

fn call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: json!({ "path": "README.md" }).as_object().unwrap().clone(),
        thought_signature: None,
    }
}

fn result(id: &str, text: &str, is_error: bool) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: id.to_string(),
        tool_name: "read".to_string(),
        content: vec![
            TextContent {
                text: text.to_string(),
                text_signature: None,
            }
            .into(),
        ],
        is_error,
        ..ToolResultMessage::default()
    }
}

fn assistant(calls: Vec<ToolCall>) -> AssistantMessage {
    AssistantMessage {
        content: calls.into_iter().map(AssistantContent::from).collect(),
        ..AssistantMessage::default()
    }
}

fn user(text: &str) -> UserMessage {
    UserMessage {
        content: text.into(),
        ..UserMessage::default()
    }
}

fn read_call() -> ToolCall {
    call("call-1", "read")
}

fn read_result() -> ToolResultMessage {
    result("call-1", "contents", false)
}

#[test]
fn valid_tool_history_is_unchanged() {
    let messages: Vec<AgentMessage> = vec![
        user("read").into(),
        assistant(vec![read_call()]).into(),
        read_result().into(),
        assistant(vec![]).into(),
    ];

    let repair = repair_tool_history(&messages);

    assert!(!repair.changed);
    assert_eq!(repair.messages, messages);
    assert_eq!(
        repair.diagnostic_data(),
        json!({
            "synthesizedResults": 0,
            "droppedOrphanResults": 0,
            "droppedDuplicateResults": 0,
            "reorderedResults": 0,
        })
    );
}

#[test]
fn a_missing_result_gets_a_synthetic_interruption() {
    let call = read_call();

    let repair = repair_tool_history(&[user("read").into(), assistant(vec![call.clone()]).into()]);

    assert!(repair.changed);
    assert_eq!(repair.messages.len(), 3);
    let AgentMessage::ToolResult(result) = &repair.messages[2] else {
        panic!("the repair must append a tool result");
    };
    assert_eq!(result.tool_call_id, call.id);
    assert_eq!(result.tool_name, call.name);
    assert_eq!(result.text(), INTERRUPTED_TOOL_RESULT);
    assert!(result.is_error);
    assert_eq!(repair.synthesized_results, 1);
}

#[test]
fn an_orphan_result_is_dropped_without_inventing_a_call() {
    let orphan: AgentMessage = result("call-missing", "contents", false).into();
    let user_message: AgentMessage = user("continue").into();

    let repair = repair_tool_history(&[orphan, user_message.clone()]);

    assert!(repair.changed);
    assert_eq!(repair.messages, vec![user_message]);
    assert_eq!(repair.dropped_orphan_results, 1);
    assert_eq!(repair.synthesized_results, 0);
}

#[test]
fn a_late_result_moves_next_to_its_call() {
    let assistant_message: AgentMessage = assistant(vec![read_call()]).into();
    let user_message: AgentMessage = user("continue").into();
    let late: AgentMessage = read_result().into();

    let repair = repair_tool_history(&[
        assistant_message.clone(),
        user_message.clone(),
        late.clone(),
    ]);

    assert_eq!(repair.messages, vec![assistant_message, late, user_message]);
    assert!(repair.changed);
    assert_eq!(repair.reordered_results, 1);
}

#[test]
fn duplicate_results_prefer_real_output_over_an_interruption() {
    let assistant_message: AgentMessage = assistant(vec![read_call()]).into();
    let interrupted: AgentMessage = result("call-1", INTERRUPTED_TOOL_RESULT, true).into();
    let actual: AgentMessage = result("call-1", "contents", false).into();

    let repair = repair_tool_history(&[assistant_message.clone(), interrupted, actual.clone()]);

    assert_eq!(repair.messages, vec![assistant_message, actual]);
    assert_eq!(repair.dropped_duplicate_results, 1);
}

#[test]
fn repeated_call_ids_keep_one_result_per_turn() {
    let messages: Vec<AgentMessage> = vec![
        assistant(vec![call("same", "read")]).into(),
        result("same", "first", false).into(),
        assistant(vec![call("same", "read")]).into(),
        result("same", "second", false).into(),
    ];

    let repair = repair_tool_history(&messages);

    assert!(!repair.changed);
    assert_eq!(repair.messages, messages);
    assert_eq!(repair.dropped_duplicate_results, 0);
}

#[test]
fn a_repeated_call_id_reserves_the_later_adjacent_result() {
    let first_assistant: AgentMessage = assistant(vec![call("same", "read")]).into();
    let user_message: AgentMessage = user("continue").into();
    let second_assistant: AgentMessage = assistant(vec![call("same", "read")]).into();
    let second_result: AgentMessage = result("same", "second", false).into();

    let repair = repair_tool_history(&[
        first_assistant.clone(),
        user_message.clone(),
        second_assistant.clone(),
        second_result.clone(),
    ]);

    let first_result = repair.messages[1].clone();
    let AgentMessage::ToolResult(synthesized) = &first_result else {
        panic!("the first call must be paired with a synthesized result");
    };
    assert_eq!(synthesized.text(), INTERRUPTED_TOOL_RESULT);
    assert_eq!(repair.synthesized_results, 1);
    assert_eq!(
        repair.messages,
        vec![
            first_assistant,
            first_result,
            user_message,
            second_assistant,
            second_result,
        ]
    );
}

#[test]
fn parallel_results_follow_the_tool_call_order() {
    let first = call("call-1", "read");
    let second = call("call-2", "bash");
    let assistant_message: AgentMessage = assistant(vec![first.clone(), second.clone()]).into();
    let second_result: AgentMessage = ToolResultMessage {
        tool_call_id: "call-2".to_string(),
        tool_name: "bash".to_string(),
        content: vec![
            TextContent {
                text: "second".to_string(),
                text_signature: None,
            }
            .into(),
        ],
        ..ToolResultMessage::default()
    }
    .into();
    let first_result: AgentMessage = result("call-1", "first", false).into();

    let repair = repair_tool_history(&[
        assistant_message.clone(),
        second_result.clone(),
        first_result.clone(),
    ]);

    assert_eq!(
        repair.messages,
        vec![assistant_message, first_result, second_result]
    );
}
