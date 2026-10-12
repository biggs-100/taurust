//! The stateful harness: queues, subscribers, cancellation and transcript repair.
//!
//! Mirrors tau's `tests/test_agent_harness.py`. The reference's fake provider
//! lives in `tau_ai`, which this crate must never depend on, so this file defines
//! its own, as the loop's tests do.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use serde_json::json;
use tau_agent::async_trait;
use tau_agent::harness::{AgentHarness, AgentHarnessConfig, HarnessError, QueueMode, Subscription};
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_protocol::content::{AssistantContent, TextContent, ToolCall};
use tau_protocol::message::{AgentMessage, AssistantMessage, UserMessage};
use tau_protocol::provider::{ModelProvider, ModelRequest, ProviderStream};
use tau_protocol::provider_event::{AssistantMessageEvent, DoneReason};
use tau_protocol::{AgentEvent, AgentToolResult, JsonObject, ToolSpec};

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

    fn session_ids(&self) -> Vec<Option<String>> {
        self.requests()
            .iter()
            .map(|request| request.session_id.clone())
            .collect()
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
        arguments: json!({ "text": "hi" }).as_object().unwrap().clone(),
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
    AssistantMessageEvent::Done { reason, message }
}

fn collect(stream: tau_agent::agent_loop::AgentStream) -> Vec<AgentEvent> {
    futures_executor::block_on(stream.collect::<Vec<_>>())
}

fn kinds(events: &[AgentEvent]) -> Vec<&str> {
    events.iter().map(AgentEvent::kind).collect()
}

fn transcript(harness: &AgentHarness) -> Vec<(String, String)> {
    harness
        .messages()
        .iter()
        .map(|message| (message.role().to_string(), message.text()))
        .collect()
}

fn harness(provider: Arc<TestProvider>) -> AgentHarness {
    AgentHarness::new(AgentHarnessConfig::new(provider, "fake", "You are Tau."))
}

/// Coerce the test provider to the trait object the harness wants, keeping the
/// concrete handle for assertions.
fn as_provider(provider: &Arc<TestProvider>) -> Arc<dyn ModelProvider> {
    Arc::clone(provider) as Arc<dyn ModelProvider>
}

/// A tool that echoes its `text` argument.
struct EchoTool {
    spec: ToolSpec,
}

#[async_trait]
impl Tool for EchoTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        Ok(AgentToolResult::from(
            arguments
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or_default(),
        ))
    }
}

/// A tool that never finishes, so a run can only end by being abandoned.
struct HangingTool {
    spec: ToolSpec,
}

#[async_trait]
impl Tool for HangingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        _arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        std::future::pending::<Result<AgentToolResult, ToolError>>().await
    }
}

fn tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        label: name.to_string(),
        description: format!("{name} tool"),
        parameters: json!({ "type": "object" }),
        prompt_snippet: None,
        prompt_guidelines: Vec::new(),
    }
}

#[test]
fn a_prompt_appends_the_user_and_assistant_with_the_pi_lifecycle() {
    let provider = TestProvider::new(vec![vec![
        start(),
        done(partial("Hello"), DoneReason::Stop),
    ]]);
    let harness = harness(provider);

    let events = collect(harness.prompt("Hi").expect("the run starts"));

    assert_eq!(
        kinds(&events),
        vec![
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );

    let starts: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageStart { message } => Some(message.role()),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec!["user", "assistant"]);

    assert_eq!(
        transcript(&harness),
        vec![
            ("user".to_string(), "Hi".to_string()),
            ("assistant".to_string(), "Hello".to_string()),
        ]
    );
}

#[test]
fn subscribers_receive_nested_updates_and_unsubscribe() {
    let assistant = partial("Hello");
    let provider = TestProvider::new(vec![
        vec![
            start(),
            text_delta("Hello"),
            done(assistant.clone(), DoneReason::Stop),
        ],
        vec![start(), done(assistant, DoneReason::Stop)],
    ]);
    let harness = AgentHarness::new(AgentHarnessConfig {
        session_id: Some("session-123".to_string()),
        ..AgentHarnessConfig::new(as_provider(&provider), "fake", "You are Tau.")
    });

    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorder = Arc::clone(&seen);
    let subscription: Subscription = harness.subscribe(Arc::new(move |event: AgentEvent| {
        recorder
            .lock()
            .expect("seen lock")
            .push(event.kind().to_string());
    }));

    let _ = collect(harness.prompt("Hi").expect("the first run starts"));
    subscription.unsubscribe();
    let _ = collect(harness.continue_().expect("the second run starts"));

    let captured = seen.lock().expect("seen lock").clone();
    assert!(captured.contains(&"message_update".to_string()));
    assert_eq!(captured.last().map(String::as_str), Some("agent_end"));
    assert_eq!(
        provider.requests().len(),
        2,
        "unsubscribing must not stop the run"
    );
    assert_eq!(
        provider.session_ids(),
        vec![
            Some("session-123".to_string()),
            Some("session-123".to_string())
        ]
    );
}

#[test]
fn the_harness_rejects_an_overlapping_run_and_drains_follow_ups() {
    let provider = TestProvider::new(vec![
        vec![start(), done(partial("First"), DoneReason::Stop)],
        vec![start(), done(partial("Second"), DoneReason::Stop)],
    ]);
    let harness = harness(provider);

    let mut stream = harness.prompt("Hi").expect("the run starts");
    let mut queued = false;
    futures_executor::block_on(async {
        while let Some(event) = stream.next().await {
            if let AgentEvent::MessageStart { message } = &event
                && message.role() == "assistant"
                && !queued
            {
                assert!(matches!(
                    harness.prompt("overlap"),
                    Err(HarnessError::AlreadyRunning)
                ));
                harness.follow_up("Later");
                queued = true;
            }
        }
    });

    assert_eq!(
        transcript(&harness),
        vec![
            ("user".to_string(), "Hi".to_string()),
            ("assistant".to_string(), "First".to_string()),
            ("user".to_string(), "Later".to_string()),
            ("assistant".to_string(), "Second".to_string()),
        ]
    );
    assert!(!harness.is_running());
}

#[test]
fn the_all_queue_mode_drains_the_messages_together() {
    let provider = TestProvider::new(vec![
        vec![start(), done(partial("First"), DoneReason::Stop)],
        vec![start(), done(partial("Second"), DoneReason::Stop)],
    ]);
    let harness = AgentHarness::new(AgentHarnessConfig {
        queue_mode: QueueMode::All,
        ..AgentHarnessConfig::new(provider, "fake", "You are Tau.")
    });

    let mut stream = harness.prompt("Hi").expect("the run starts");
    futures_executor::block_on(async {
        while let Some(event) = stream.next().await {
            if let AgentEvent::MessageEnd { message } = &event
                && message.role() == "assistant"
                && message.text() == "First"
            {
                harness.follow_up("Second prompt");
                harness.follow_up("Third prompt");
            }
        }
    });

    let users: Vec<String> = harness
        .messages()
        .iter()
        .filter(|message| message.role() == "user")
        .map(AgentMessage::text)
        .collect();
    assert_eq!(users, vec!["Hi", "Second prompt", "Third prompt"]);
}

#[test]
fn the_harness_passes_its_tools_to_the_loop() {
    let tool_call = call("call-1", "echo");
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
        vec![start(), done(partial("Done"), DoneReason::Stop)],
    ]);
    let harness = AgentHarness::new(AgentHarnessConfig {
        tools: vec![Arc::new(EchoTool {
            spec: tool_spec("echo"),
        })],
        ..AgentHarnessConfig::new(as_provider(&provider), "fake", "You are Tau.")
    });

    let _ = collect(harness.prompt("echo").expect("the run starts"));

    let result = harness
        .messages()
        .into_iter()
        .find_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result),
            _ => None,
        })
        .expect("a tool result is recorded");
    assert_eq!(result.tool_name, "echo");
    assert_eq!(result.text(), "hi");

    let first_request = provider.requests().into_iter().next().expect("a request");
    let names: Vec<String> = first_request
        .tools
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    assert_eq!(names, vec!["echo"]);
}

#[test]
fn the_queue_mutators_return_snapshots() {
    let harness = harness(TestProvider::new(vec![]));

    harness.steer("First");
    let second = harness
        .steer("Second")
        .steering
        .last()
        .cloned()
        .expect("the second steering message");
    let later = harness
        .follow_up("Later")
        .follow_up
        .last()
        .cloned()
        .expect("the follow-up message");

    assert_eq!(harness.pop_latest_steering(), Some(second));
    assert_eq!(harness.pop_latest_follow_up(), Some(later));
    assert_eq!(
        harness
            .queued_messages()
            .steering
            .iter()
            .map(AgentMessage::text)
            .collect::<Vec<_>>(),
        vec!["First"]
    );

    let cleared = harness.clear_queues();
    assert_eq!(
        cleared
            .steering
            .iter()
            .map(AgentMessage::text)
            .collect::<Vec<_>>(),
        vec!["First"]
    );
    assert_eq!(harness.pending_message_count(), 0);
}

#[test]
fn a_repaired_transcript_is_reported_before_the_run() {
    let tool_call = call("call-1", "echo");
    let mut dangling = AssistantMessage {
        model: "fake".to_string(),
        ..AssistantMessage::default()
    };
    dangling.content.push(AssistantContent::ToolCall(tool_call));
    let provider = TestProvider::new(vec![vec![start(), done(partial("Done"), DoneReason::Stop)]]);
    let harness = AgentHarness::with_messages(
        AgentHarnessConfig::new(provider, "fake", "You are Tau."),
        vec![dangling.into()],
    );

    let _ = collect(harness.prompt("Hi").expect("the run starts"));

    let repairs: Vec<String> = harness
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult(result) => Some(result.text()),
            _ => None,
        })
        .collect();
    assert_eq!(repairs, vec!["Tool call interrupted by user"]);
}

#[test]
fn abandoning_a_cancelled_run_repairs_the_tool_call_that_never_returned() {
    let tool_call = call("call-1", "hang");
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
    let harness = AgentHarness::new(AgentHarnessConfig {
        tools: vec![Arc::new(HangingTool {
            spec: tool_spec("hang"),
        })],
        ..AgentHarnessConfig::new(provider, "fake", "You are Tau.")
    });

    {
        let mut stream = harness.prompt("work").expect("the run starts");
        loop {
            let event = futures_executor::block_on(stream.next()).expect("the run emits an event");
            if matches!(event, AgentEvent::ToolExecutionStart { .. }) {
                break;
            }
        }
        harness.cancel();
    }

    assert!(
        !harness.is_running(),
        "abandoning the run must clear the flag"
    );
    let results: Vec<(String, bool)> = harness
        .messages()
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult(result) => Some((result.text(), result.is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![("Tool call interrupted by user".to_string(), true)]
    );
}

#[test]
fn the_prompt_message_helper_accepts_any_message() {
    let provider = TestProvider::new(vec![vec![
        start(),
        done(partial("Hello"), DoneReason::Stop),
    ]]);
    let harness = harness(provider);

    let message: AgentMessage = UserMessage {
        content: "from a frontend".into(),
        ..UserMessage::default()
    }
    .into();
    let _ = collect(harness.prompt_message(message).expect("the run starts"));

    assert_eq!(transcript(&harness)[0].1, "from a frontend");
}
