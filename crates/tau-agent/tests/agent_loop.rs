//! The provider/tool agent loop.
//!
//! Mirrors tau's `tests/test_agent_loop.py`. The reference's own fake provider
//! lives in `tau_ai`, which this crate must never depend on, so the tests define
//! a small one here.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use serde_json::json;
use tau_agent::agent_loop::{
    AfterToolCall, AgentLoopConfig, AgentStream, BeforeToolCall, run_agent_loop,
};
use tau_agent::async_trait;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_protocol::content::{AssistantContent, TextContent, ToolCall};
use tau_protocol::message::{AgentMessage, AssistantMessage, StopReason, UserMessage};
use tau_protocol::provider::{
    CancellationToken, ManualCancellation, ModelProvider, ModelRequest, ProviderStream,
};
use tau_protocol::provider_event::{AssistantMessageEvent, DoneReason};
use tau_protocol::{AgentEvent, AgentToolResult, JsonObject, ToolSpec};

/// A provider that replays fixed streams and records what it was asked for.
struct TestProvider {
    streams: Mutex<VecDeque<Vec<AssistantMessageEvent>>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl TestProvider {
    fn new(streams: Vec<Vec<AssistantMessageEvent>>) -> Arc<Self> {
        Arc::new(Self {
            streams: Mutex::new(streams.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl ModelProvider for TestProvider {
    fn stream_response(&self, request: ModelRequest) -> ProviderStream {
        self.requests
            .lock()
            .expect("requests lock")
            .push(request.clone());
        let events = self
            .streams
            .lock()
            .expect("streams lock")
            .pop_front()
            .unwrap_or_default();
        Box::pin(futures_util::stream::iter(events))
    }
}

fn user(text: &str) -> UserMessage {
    UserMessage {
        content: text.into(),
        ..UserMessage::default()
    }
}

fn text(value: &str) -> TextContent {
    TextContent {
        text: value.to_string(),
        text_signature: None,
    }
}

fn partial(value: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![text(value).into()],
        model: "fake".to_string(),
        ..AssistantMessage::default()
    }
}

fn call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: json!({ "path": "README.md" }).as_object().unwrap().clone(),
        thought_signature: None,
    }
}

fn start() -> AssistantMessageEvent {
    AssistantMessageEvent::Start {
        partial: AssistantMessage {
            model: "fake".to_string(),
            ..AssistantMessage::default()
        },
    }
}

fn text_delta(delta: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::TextDelta {
        content_index: 0,
        delta: delta.to_string(),
        partial: partial(delta),
    }
}

fn thinking_delta(delta: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::ThinkingDelta {
        content_index: 0,
        delta: delta.to_string(),
        partial: AssistantMessage {
            content: vec![
                tau_protocol::content::ThinkingContent {
                    thinking: delta.to_string(),
                    thinking_signature: None,
                    redacted: false,
                }
                .into(),
            ],
            model: "fake".to_string(),
            ..AssistantMessage::default()
        },
    }
}

fn tool_call_end(tool_call: &ToolCall) -> AssistantMessageEvent {
    AssistantMessageEvent::ToolCallEnd {
        content_index: 0,
        tool_call: tool_call.clone(),
        partial: AssistantMessage {
            content: vec![AssistantContent::ToolCall(tool_call.clone())],
            model: "fake".to_string(),
            ..AssistantMessage::default()
        },
    }
}

fn done(message: AssistantMessage, reason: DoneReason) -> AssistantMessageEvent {
    let mut message = message;
    message.stop_reason = match reason {
        DoneReason::Stop => StopReason::Stop,
        DoneReason::Length => StopReason::Length,
        DoneReason::ToolUse => StopReason::ToolUse,
    };
    AssistantMessageEvent::Done { reason, message }
}

fn collect(stream: AgentStream) -> Vec<AgentEvent> {
    futures_executor::block_on(stream.collect::<Vec<_>>())
}

fn kinds(events: &[AgentEvent]) -> Vec<&str> {
    events.iter().map(AgentEvent::kind).collect()
}

fn message_of(event: &AgentEvent) -> Option<&AgentMessage> {
    match event {
        AgentEvent::MessageStart { message }
        | AgentEvent::MessageEnd { message }
        | AgentEvent::MessageUpdate { message, .. } => Some(message),
        _ => None,
    }
}

/// A tool that echoes its `text` argument and reports one partial result.
struct EchoTool {
    spec: ToolSpec,
    seen: Mutex<Vec<(String, bool)>>,
}

impl EchoTool {
    fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            spec: ToolSpec {
                name: name.to_string(),
                label: name.to_string(),
                description: format!("{name} tool"),
                parameters: json!({ "type": "object" }),
                prompt_snippet: None,
                prompt_guidelines: Vec::new(),
            },
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<(String, bool)> {
        self.seen.lock().expect("seen lock").clone()
    }
}

#[async_trait]
impl Tool for EchoTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &JsonObject,
        updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        self.seen
            .lock()
            .expect("seen lock")
            .push((call_id.to_string(), arguments.contains_key("path")));
        updates.emit(AgentToolResult::from("working"));
        Ok(AgentToolResult::from(
            arguments
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or("done"),
        ))
    }
}

/// A clock the test drives, mirroring the reference's monkeypatched `monotonic_ns`.
fn scripted_clock(ticks: Vec<u64>) -> Arc<dyn Fn() -> u64 + Send + Sync> {
    let ticks = Arc::new(Mutex::new(VecDeque::from(ticks)));
    let last = Arc::new(Mutex::new(0u64));
    Arc::new(
        move || match ticks.lock().expect("ticks lock").pop_front() {
            Some(tick) => {
                *last.lock().expect("last lock") = tick;
                tick
            }
            None => *last.lock().expect("last lock"),
        },
    )
}

#[test]
fn the_loop_streams_the_canonical_nested_events() {
    let messages = Arc::new(Mutex::new(vec![user("Say hello").into()]));
    let final_message = partial("Hello");
    let provider = TestProvider::new(vec![vec![
        start(),
        text_delta("Hel"),
        text_delta("lo"),
        done(final_message.clone(), DoneReason::Stop),
    ]]);

    let events = collect(run_agent_loop(AgentLoopConfig::new(
        provider,
        "fake",
        "You are Tau.",
        Arc::clone(&messages),
    )));

    assert_eq!(
        kinds(&events),
        vec![
            "agent_start",
            "turn_start",
            "message_start",
            "message_update",
            "message_update",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );

    let deltas: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => match assistant_message_event.as_ref() {
                AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["Hel", "lo"]);

    let stored = messages.lock().expect("messages lock").clone();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].role(), "user");
    assert_eq!(stored[1].role(), "assistant");
}

#[test]
fn the_loop_records_only_provider_wait_time() {
    let messages = Arc::new(Mutex::new(vec![user("Say hello").into()]));
    let final_message = partial("Hello");
    let provider = TestProvider::new(vec![vec![
        start(),
        text_delta("Hello"),
        done(final_message.clone(), DoneReason::Stop),
    ]]);

    let events = collect(run_agent_loop(AgentLoopConfig {
        clock: Some(scripted_clock(vec![
            0,
            100_000_000,
            1_000_000_000,
            2_150_000_000,
            10_000_000_000,
            13_750_000_000,
        ])),
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    let AgentEvent::AgentEnd { .. } = events.last().expect("the loop ends") else {
        panic!("the loop must end with agent_end");
    };
    let stored = messages.lock().expect("messages lock").clone();
    let timing = match &stored[1] {
        AgentMessage::Assistant(assistant) => assistant.timing.expect("timing is recorded"),
        other => panic!("expected an assistant message, got {other:?}"),
    };

    assert_eq!(timing.time_to_first_output_ms, Some(1250));
    assert_eq!(timing.total_duration_ms, 5000);
}

#[test]
fn the_loop_does_not_invent_a_first_output_time() {
    let messages = Arc::new(Mutex::new(vec![user("Say hello").into()]));
    let provider = TestProvider::new(vec![vec![
        start(),
        done(partial("Non-streamed"), DoneReason::Stop),
    ]]);

    let _ = collect(run_agent_loop(AgentLoopConfig {
        clock: Some(scripted_clock(vec![
            0,
            100_000_000,
            1_000_000_000,
            2_000_000_000,
        ])),
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    let stored = messages.lock().expect("messages lock").clone();
    let AgentMessage::Assistant(assistant) = &stored[1] else {
        panic!("expected an assistant message");
    };
    let timing = assistant.timing.expect("timing is recorded");

    assert_eq!(timing.time_to_first_output_ms, None);
    assert_eq!(timing.total_duration_ms, 1100);
}

#[test]
fn the_loop_nests_thinking_events_without_losing_the_final_message() {
    let messages = Arc::new(Mutex::new(vec![user("Think briefly").into()]));
    let final_message = partial("Done");
    let provider = TestProvider::new(vec![vec![
        start(),
        thinking_delta("hidden "),
        thinking_delta("reasoning"),
        text_delta("Done"),
        done(final_message, DoneReason::Stop),
    ]]);

    let events = collect(run_agent_loop(AgentLoopConfig::new(
        provider,
        "fake",
        "You are Tau.",
        Arc::clone(&messages),
    )));

    let nested: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => match assistant_message_event.as_ref() {
                AssistantMessageEvent::ThinkingDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(nested, vec!["hidden ", "reasoning"]);

    let stored = messages.lock().expect("messages lock").clone();
    assert_eq!(stored.last().expect("a final message").role(), "assistant");
    assert_eq!(stored.last().expect("a final message").text(), "Done");
}

#[test]
fn the_loop_executes_a_tool_and_emits_the_result_message_lifecycle() {
    let messages = Arc::new(Mutex::new(vec![user("Read README.md").into()]));
    let tool_call = call("call-1", "read");
    let mut first = partial("Reading.");
    first
        .content
        .push(AssistantContent::ToolCall(tool_call.clone()));
    let final_message = partial("Done.");
    let provider = TestProvider::new(vec![
        vec![
            start(),
            tool_call_end(&tool_call),
            done(first, DoneReason::ToolUse),
        ],
        vec![
            start(),
            text_delta("Done."),
            done(final_message, DoneReason::Stop),
        ],
    ]);
    let tool = EchoTool::new("read");
    let requests = Arc::clone(&provider);

    let events = collect(run_agent_loop(AgentLoopConfig {
        tools: vec![Arc::clone(&tool) as Arc<dyn Tool>],
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    let stored = messages.lock().expect("messages lock").clone();
    let result = stored
        .iter()
        .find_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .expect("a tool result is recorded");

    assert_eq!(result.tool_name, "read");
    assert_eq!(result.text(), "README.md");
    assert!(!result.is_error);

    let result_lifecycle = events
        .iter()
        .filter(|event| {
            matches!(event, AgentEvent::MessageEnd { message } if message.role() == "toolResult")
        })
        .count();
    assert_eq!(result_lifecycle, 1);
    assert_eq!(
        kinds(&events)
            .iter()
            .filter(|kind| **kind == "message_start")
            .count(),
        3
    );

    let second_request = requests
        .requests()
        .get(1)
        .expect("a second request")
        .clone();
    assert_eq!(second_request.messages.len(), 3);
    assert_eq!(second_request.messages[2].role(), "toolResult");
}

#[test]
fn the_loop_passes_the_call_id_and_reports_tool_progress() {
    let messages = Arc::new(Mutex::new(vec![user("work").into()]));
    let tool_call = call("call-1", "work");
    let mut first = AssistantMessage {
        model: "fake".to_string(),
        ..AssistantMessage::default()
    };
    first
        .content
        .push(AssistantContent::ToolCall(tool_call.clone()));
    let provider = TestProvider::new(vec![
        vec![
            start(),
            tool_call_end(&tool_call),
            done(first, DoneReason::ToolUse),
        ],
        vec![start(), done(partial("finished"), DoneReason::Stop)],
    ]);
    let tool = EchoTool::new("work");

    let events = collect(run_agent_loop(AgentLoopConfig {
        tools: vec![Arc::clone(&tool) as Arc<dyn Tool>],
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    assert_eq!(tool.seen(), vec![("call-1".to_string(), true)]);
    let updates: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionUpdate { partial_result, .. } => Some(partial_result.text()),
            _ => None,
        })
        .collect();
    assert_eq!(updates, vec!["working"]);
}

#[test]
fn the_loop_records_an_unknown_tool_as_an_error_result() {
    let messages = Arc::new(Mutex::new(vec![user("Use it").into()]));
    let tool_call = call("call-1", "missing");
    let mut first = AssistantMessage {
        model: "fake".to_string(),
        ..AssistantMessage::default()
    };
    first
        .content
        .push(AssistantContent::ToolCall(tool_call.clone()));
    let provider = TestProvider::new(vec![vec![
        start(),
        tool_call_end(&tool_call),
        done(first, DoneReason::ToolUse),
    ]]);

    let _ = collect(run_agent_loop(AgentLoopConfig {
        max_turns: Some(1),
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    let stored = messages.lock().expect("messages lock").clone();
    let result = stored
        .iter()
        .find_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .expect("a tool result is recorded");

    assert_eq!(result.text(), "Tool missing not found");
    assert!(result.is_error);
}

#[test]
fn the_loop_stops_after_the_turn_limit() {
    let messages = Arc::new(Mutex::new(vec![user("Use it").into()]));
    let tool_call = call("call-1", "missing");
    let mut first = AssistantMessage {
        model: "fake".to_string(),
        ..AssistantMessage::default()
    };
    first
        .content
        .push(AssistantContent::ToolCall(tool_call.clone()));
    let provider = TestProvider::new(vec![vec![
        start(),
        tool_call_end(&tool_call),
        done(first, DoneReason::ToolUse),
    ]]);

    let events = collect(run_agent_loop(AgentLoopConfig {
        max_turns: Some(1),
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    let stop_message = events
        .iter()
        .filter_map(message_of)
        .find_map(|message| match message {
            AgentMessage::Assistant(assistant) => assistant.error_message.clone(),
            _ => None,
        })
        .expect("the loop reports why it stopped");

    assert_eq!(stop_message, "Agent stopped after max_turns=1");
    assert_eq!(kinds(&events).last(), Some(&"agent_end"));
}

#[test]
fn a_cancelled_signal_aborts_a_tool_instead_of_running_it() {
    let messages = Arc::new(Mutex::new(vec![user("work").into()]));
    let tool_call = call("call-1", "work");
    let mut first = AssistantMessage {
        model: "fake".to_string(),
        ..AssistantMessage::default()
    };
    first
        .content
        .push(AssistantContent::ToolCall(tool_call.clone()));
    let provider = TestProvider::new(vec![vec![
        start(),
        tool_call_end(&tool_call),
        done(first, DoneReason::ToolUse),
    ]]);
    let tool = EchoTool::new("work");
    let signal = Arc::new(ManualCancellation::default());
    signal.cancel();

    let events = collect(run_agent_loop(AgentLoopConfig {
        tools: vec![Arc::clone(&tool) as Arc<dyn Tool>],
        signal: Some(Arc::clone(&signal) as Arc<dyn CancellationToken>),
        max_turns: Some(1),
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    assert!(
        tool.seen().is_empty(),
        "a cancelled run must not call the tool"
    );
    let result = messages
        .lock()
        .expect("messages lock")
        .iter()
        .find_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .expect("a tool result is recorded");
    assert_eq!(result.text(), "Operation aborted");
    assert!(result.is_error);
    assert_eq!(kinds(&events).last(), Some(&"agent_end"));
}

#[test]
fn a_before_hook_can_block_a_tool_call() {
    let messages = Arc::new(Mutex::new(vec![user("work").into()]));
    let tool_call = call("call-1", "work");
    let mut first = AssistantMessage {
        model: "fake".to_string(),
        ..AssistantMessage::default()
    };
    first
        .content
        .push(AssistantContent::ToolCall(tool_call.clone()));
    let provider = TestProvider::new(vec![vec![
        start(),
        tool_call_end(&tool_call),
        done(first, DoneReason::ToolUse),
    ]]);
    let tool = EchoTool::new("work");
    let before: BeforeToolCall = Arc::new(|call: ToolCall| {
        Box::pin(async move { (true, Some(format!("{} is not allowed", call.name))) })
    });

    let _ = collect(run_agent_loop(AgentLoopConfig {
        tools: vec![Arc::clone(&tool) as Arc<dyn Tool>],
        before_tool_call: Some(before),
        max_turns: Some(1),
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    assert!(
        tool.seen().is_empty(),
        "a blocked tool must not be executed"
    );
    let result = messages
        .lock()
        .expect("messages lock")
        .iter()
        .find_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .expect("a tool result is recorded");
    assert_eq!(result.text(), "work is not allowed");
    assert!(result.is_error);
}

#[test]
fn an_after_hook_can_rewrite_a_tool_result() {
    let messages = Arc::new(Mutex::new(vec![user("work").into()]));
    let tool_call = call("call-1", "work");
    let mut first = AssistantMessage {
        model: "fake".to_string(),
        ..AssistantMessage::default()
    };
    first
        .content
        .push(AssistantContent::ToolCall(tool_call.clone()));
    let provider = TestProvider::new(vec![vec![
        start(),
        tool_call_end(&tool_call),
        done(first, DoneReason::ToolUse),
    ]]);
    let tool = EchoTool::new("work");
    let after: AfterToolCall = Arc::new(|_call: ToolCall, _result, _is_error| {
        Box::pin(async move { (AgentToolResult::from("rewritten"), false) })
    });

    let events = collect(run_agent_loop(AgentLoopConfig {
        tools: vec![Arc::clone(&tool) as Arc<dyn Tool>],
        after_tool_call: Some(after),
        max_turns: Some(1),
        ..AgentLoopConfig::new(provider, "fake", "You are Tau.", Arc::clone(&messages))
    }));

    let result = messages
        .lock()
        .expect("messages lock")
        .iter()
        .find_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.clone()),
            _ => None,
        })
        .expect("a tool result is recorded");
    assert_eq!(result.text(), "rewritten");
    assert!(!result.is_error);

    let end = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionEnd { result, .. } => Some(result.text()),
            _ => None,
        })
        .expect("the tool execution ends");
    assert_eq!(
        end, "rewritten",
        "the end event reports the rewritten result"
    );
}
