//! The persisted session JSONL format.
//!
//! Mirrors tau's `tests/test_session.py` for the entry-level facts: the persisted
//! shape omits `null` model fields, keeps snake_case entry field names, and
//! embeds transcript messages in their own camelCase shape.

use serde_json::json;
use tau_agent::session::entries::{EntryBody, SessionEntry};
use tau_agent::session::jsonl::{
    SessionJsonlError, entries_from_json_lines, entry_from_json_line, entry_to_json_line,
};
use tau_protocol::content::{ImageContent, TextContent, ToolCall};
use tau_protocol::message::{
    AgentMessage, AssistantMessage, ResponseTiming, ToolResultMessage, Usage, UsageCost,
    UserMessage,
};
use tau_protocol::{JsonValue, UserContent};

fn line_of(entry: &SessionEntry) -> String {
    entry_to_json_line(entry).expect("the entry serializes")
}

fn payload(entry: &SessionEntry) -> JsonValue {
    serde_json::from_str(line_of(entry).trim_end()).expect("the line is JSON")
}

fn user(text: &str) -> AgentMessage {
    UserMessage {
        content: text.into(),
        timestamp: 2,
        ..UserMessage::default()
    }
    .into()
}

fn message_entry(message: AgentMessage) -> SessionEntry {
    SessionEntry::explicit("entry-1", 1.0, EntryBody::Message { message })
}

#[test]
fn a_message_entry_round_trips_the_canonical_jsonl() {
    let entry = message_entry(user("Hello"));
    let line = line_of(&entry);

    assert_eq!(
        entry_from_json_line(&line, None).expect("the line decodes"),
        entry
    );
    assert_eq!(
        payload(&entry)["message"],
        json!({ "role": "user", "content": "Hello", "timestamp": 2 })
    );
}

#[test]
fn entry_field_names_stay_snake_case_while_message_fields_are_camel_case() {
    let entry = SessionEntry::explicit(
        "entry-1",
        2.0,
        EntryBody::CustomMessage {
            custom_type: "subagent-notification".to_string(),
            content: UserContent::Blocks(vec![
                TextContent {
                    text: "<task-notification/>".to_string(),
                    text_signature: None,
                }
                .into(),
                ImageContent {
                    data: "aW1hZ2U=".to_string(),
                    mime_type: "image/png".to_string(),
                }
                .into(),
            ]),
            display: true,
            details: Some(json!({ "id": "run-1" })),
        },
    );

    let payload = payload(&entry);

    assert_eq!(payload["type"], json!("custom_message"));
    assert_eq!(payload["custom_type"], json!("subagent-notification"));
    assert_eq!(payload.get("customType"), None);
    assert_eq!(payload["content"][1]["mimeType"], json!("image/png"));
    assert_eq!(
        entry_from_json_line(&line_of(&entry), None).expect("the line decodes"),
        entry
    );
}

#[test]
fn an_assistant_and_a_tool_result_round_trip_their_blocks() {
    let assistant = message_entry(
        AssistantMessage {
            content: vec![
                TextContent {
                    text: "Hi".to_string(),
                    text_signature: None,
                }
                .into(),
            ],
            ..AssistantMessage::default()
        }
        .into(),
    );
    let result = message_entry(
        ToolResultMessage {
            tool_call_id: "call-1".to_string(),
            tool_name: "edit".to_string(),
            content: vec![
                TextContent {
                    text: "Successfully replaced 1 block.".to_string(),
                    text_signature: None,
                }
                .into(),
            ],
            details: Some(json!({ "patch": "--- a.py\n+++ a.py" })),
            ..ToolResultMessage::default()
        }
        .into(),
    );

    let assistant_payload = payload(&assistant);
    let result_payload = payload(&result);

    assert_eq!(
        assistant_payload["message"]["content"][0]["text"],
        json!("Hi")
    );
    assert_eq!(
        assistant_payload["message"]["usage"]["totalTokens"],
        json!(0)
    );
    assert_eq!(result_payload["message"]["role"], json!("toolResult"));
    assert_eq!(result_payload["message"]["toolName"], json!("edit"));
    assert_eq!(
        entry_from_json_line(&line_of(&assistant), None).expect("the line decodes"),
        assistant
    );
    assert_eq!(
        entry_from_json_line(&line_of(&result), None).expect("the line decodes"),
        result
    );
}

#[test]
fn a_null_model_field_is_omitted_rather_than_written() {
    let entry = message_entry(
        AssistantMessage {
            content: vec![
                TextContent {
                    text: "Hi".to_string(),
                    text_signature: None,
                }
                .into(),
            ],
            timing: None,
            ..AssistantMessage::default()
        }
        .into(),
    );

    let message = payload(&entry)["message"].clone();

    assert!(
        message.get("timing").is_none(),
        "the persisted shape omits a null model field"
    );
    assert!(
        message["content"][0].get("textSignature").is_none(),
        "and it does so inside content blocks too"
    );
}

#[test]
fn a_null_inside_a_free_form_payload_survives() {
    // The reference only excludes model fields, so a null a caller put inside
    // `details` is preserved. This pins that boundary rather than leaving it to
    // chance.
    let entry = message_entry(
        ToolResultMessage {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            details: Some(json!({ "kept": null, "present": 1 })),
            ..ToolResultMessage::default()
        }
        .into(),
    );

    let details = payload(&entry)["message"]["details"].clone();

    assert_eq!(details, json!({ "kept": null, "present": 1 }));
}

#[test]
fn response_timing_round_trips_with_camel_case_keys() {
    let entry = message_entry(
        AssistantMessage {
            content: vec![
                TextContent {
                    text: "Hi".to_string(),
                    text_signature: None,
                }
                .into(),
            ],
            timing: Some(ResponseTiming {
                time_to_first_output_ms: Some(250),
                total_duration_ms: 1000,
            }),
            ..AssistantMessage::default()
        }
        .into(),
    );

    assert_eq!(
        payload(&entry)["message"]["timing"],
        json!({ "timeToFirstOutputMs": 250, "totalDurationMs": 1000 })
    );
    assert_eq!(
        entry_from_json_line(&line_of(&entry), None).expect("the line decodes"),
        entry
    );
}

#[test]
fn an_empty_replaced_id_list_is_never_written() {
    let entry = SessionEntry::explicit(
        "entry-1",
        1.0,
        EntryBody::Compaction {
            summary: "compacted".to_string(),
            replaces_entry_ids: Vec::new(),
            first_kept_entry_id: Some("entry-9".to_string()),
            tokens_before: Some(100),
            usage: None,
            provider: None,
            model: None,
            response_provider: None,
        },
    );

    let payload = payload(&entry);

    assert_eq!(payload.get("replaces_entry_ids"), None);
    assert_eq!(payload["first_kept_entry_id"], json!("entry-9"));
    assert_eq!(payload.get("usage"), None);
}

#[test]
fn every_entry_type_round_trips_its_tag() {
    let entries = vec![
        message_entry(user("hello")),
        SessionEntry::explicit(
            "e2",
            2.0,
            EntryBody::CustomMessage {
                custom_type: "note".to_string(),
                content: UserContent::from("hi"),
                display: false,
                details: None,
            },
        ),
        SessionEntry::explicit(
            "e3",
            3.0,
            EntryBody::ModelChange {
                model: "claude".to_string(),
                provider: Some("anthropic".to_string()),
            },
        ),
        SessionEntry::explicit(
            "e4",
            4.0,
            EntryBody::ThinkingLevelChange {
                thinking_level: Some("high".to_string()),
            },
        ),
        SessionEntry::explicit(
            "e5",
            5.0,
            EntryBody::Compaction {
                summary: "done".to_string(),
                replaces_entry_ids: vec!["a".to_string()],
                first_kept_entry_id: None,
                tokens_before: None,
                usage: None,
                provider: None,
                model: None,
                response_provider: None,
            },
        ),
        SessionEntry::explicit(
            "e6",
            6.0,
            EntryBody::BranchSummary {
                summary: "left".to_string(),
                branch_root_id: Some("root".to_string()),
                usage: None,
                provider: None,
                model: None,
                response_provider: None,
            },
        ),
        SessionEntry::explicit(
            "e7",
            7.0,
            EntryBody::Label {
                target_id: "e1".to_string(),
                label: Some("checkpoint".to_string()),
            },
        ),
        SessionEntry::explicit(
            "e8",
            8.0,
            EntryBody::Leaf {
                entry_id: Some("e7".to_string()),
            },
        ),
        SessionEntry::explicit(
            "e9",
            9.0,
            EntryBody::SessionInfo {
                created_at: 9.0,
                cwd: Some("/tmp".to_string()),
                title: None,
            },
        ),
        SessionEntry::explicit(
            "e10",
            10.0,
            EntryBody::Custom {
                namespace: "ext".to_string(),
                data: json!({ "x": 1 }).as_object().unwrap().clone(),
            },
        ),
    ];

    let tags: Vec<&str> = entries.iter().map(SessionEntry::kind).collect();
    assert_eq!(
        tags,
        vec![
            "message",
            "custom_message",
            "model_change",
            "thinking_level_change",
            "compaction",
            "branch_summary",
            "label",
            "leaf",
            "session_info",
            "custom",
        ]
    );

    let lines: Vec<String> = entries.iter().map(line_of).collect();
    let decoded = entries_from_json_lines(lines).expect("the lines decode");

    assert_eq!(decoded, entries);
}

#[test]
fn blank_lines_are_skipped_and_line_numbers_are_reported() {
    let stored = line_of(&message_entry(user("hi")));
    let entries =
        entries_from_json_lines(["", "  ", &stored, ""]).expect("blank lines are not entries");

    assert_eq!(entries.len(), 1);

    let error = entries_from_json_lines([stored.as_str(), "not json"])
        .expect_err("the second line is not JSON");
    assert!(matches!(error, SessionJsonlError { .. }));
    assert!(
        error
            .message()
            .starts_with("Invalid session entry on line 2: "),
        "the failing line is reported: {}",
        error.message()
    );
}

#[test]
fn a_malformed_entry_reports_its_reason() {
    let error = entry_from_json_line("{\"type\":\"nonsense\"}", None)
        .expect_err("an unknown entry type is rejected");

    assert!(error.message().starts_with("Invalid session entry: "));
}

#[test]
fn a_usage_cost_round_trips_for_a_compaction_entry() {
    let entry = SessionEntry::explicit(
        "entry-1",
        1.0,
        EntryBody::Compaction {
            summary: "compacted".to_string(),
            replaces_entry_ids: Vec::new(),
            first_kept_entry_id: None,
            tokens_before: Some(1000),
            usage: Some(Usage {
                input: 10,
                output: 20,
                total_tokens: 30,
                cost: UsageCost {
                    total: 0.5,
                    ..UsageCost::default()
                },
                ..Usage::default()
            }),
            provider: Some("anthropic".to_string()),
            model: Some("claude".to_string()),
            response_provider: None,
        },
    );

    let payload = payload(&entry);
    assert_eq!(payload["usage"]["totalTokens"], json!(30));
    assert_eq!(payload["usage"]["cost"]["total"], json!(0.5));

    assert_eq!(
        entry_from_json_line(&line_of(&entry), None).expect("the line decodes"),
        entry
    );
}

#[test]
fn a_tool_call_block_round_trips_inside_a_message_entry() {
    let call = ToolCall {
        id: "call-1".to_string(),
        name: "read".to_string(),
        arguments: json!({ "path": "README.md" }).as_object().unwrap().clone(),
        thought_signature: None,
    };
    let entry = message_entry(
        AssistantMessage {
            content: vec![
                TextContent {
                    text: "Reading.".to_string(),
                    text_signature: None,
                }
                .into(),
                call.into(),
            ],
            ..AssistantMessage::default()
        }
        .into(),
    );

    let payload = payload(&entry);
    let block = &payload["message"]["content"][1];

    assert_eq!(block["type"], json!("toolCall"));
    assert_eq!(block["id"], json!("call-1"));
    assert_eq!(
        block.get("thoughtSignature"),
        None,
        "a null field is omitted here too"
    );
    assert_eq!(
        entry_from_json_line(&line_of(&entry), None).expect("the line decodes"),
        entry
    );
}
