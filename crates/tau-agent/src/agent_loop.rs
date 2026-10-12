//! The provider/tool agent loop.
//!
//! Ported from tau's `tau_agent/loop.py`. The module is called `agent_loop`
//! rather than `loop`, because `loop` is a reserved word in Rust and a module may
//! not be named after it.
//!
//! The loop drives turns until the model stops asking for tools, streaming Pi's
//! canonical events as it goes. It is the only place that decides what the
//! provider sees: the transcript is normalized through
//! [`repair_tool_history`](crate::tool_history::repair_tool_history) before every
//! request, so a half-finished turn cannot poison the next one.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::json;

use tau_protocol::content::{TextContent, ToolCall};
use tau_protocol::message::{
    AgentMessage, AssistantMessage, ResponseTiming, StopReason, ToolResultMessage,
};
use tau_protocol::provider::{CancellationToken, ModelProvider, ModelRequest};
use tau_protocol::provider_event::AssistantMessageEvent;
use tau_protocol::{AgentEvent, AgentToolResult};

use crate::tool_history::repair_tool_history;
use crate::tools::{Tool, ToolUpdates};

/// A boxed stream of agent events.
pub type AgentStream = Pin<Box<dyn Stream<Item = AgentEvent> + Send>>;

/// A veto consulted before a tool runs.
///
/// Returns whether to block the call and, when blocking, the reason to record.
/// This is the permission-gate seam: a frontend uses it to ask the user before a
/// destructive tool runs.
pub type BeforeToolCall = Arc<
    dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = (bool, Option<String>)> + Send>> + Send + Sync,
>;

/// A hook that may rewrite a tool result before it is recorded.
pub type AfterToolCall = Arc<
    dyn Fn(
            ToolCall,
            AgentToolResult,
            bool,
        ) -> Pin<Box<dyn Future<Output = (AgentToolResult, bool)> + Send>>
        + Send
        + Sync,
>;

/// Everything one run of the loop needs.
pub struct AgentLoopConfig {
    /// The model provider.
    pub provider: Arc<dyn ModelProvider>,
    /// The model name.
    pub model: String,
    /// The system prompt.
    pub system: String,
    /// The transcript the run appends to.
    ///
    /// Shared rather than owned, because the caller keeps reading it while the
    /// stream runs, exactly as the reference mutates the caller's list.
    pub messages: Arc<Mutex<Vec<AgentMessage>>>,
    /// The tools the model may call.
    pub tools: Vec<Arc<dyn Tool>>,
    /// Messages that start the run and become part of the transcript.
    pub prompts: Vec<AgentMessage>,
    /// Messages emitted before the first turn without starting it.
    pub prelude_messages: Vec<AgentMessage>,
    /// The maximum number of turns, when the run is bounded.
    pub max_turns: Option<u32>,
    /// A token the caller can cancel the run with.
    pub signal: Option<Arc<dyn CancellationToken>>,
    /// A session id, used by providers for request routing or cache affinity.
    pub session_id: Option<String>,
    /// Pulled at the start of every turn to inject messages mid-run.
    pub steering: Option<Arc<dyn Fn() -> Vec<AgentMessage> + Send + Sync>>,
    /// Pulled when the run would otherwise end, to continue it.
    pub follow_up: Option<Arc<dyn Fn() -> Vec<AgentMessage> + Send + Sync>>,
    /// Consulted before every tool call.
    pub before_tool_call: Option<BeforeToolCall>,
    /// Consulted after every tool call.
    pub after_tool_call: Option<AfterToolCall>,
    /// The monotonic clock, in nanoseconds.
    ///
    /// Injectable so timing can be tested exactly, the same way the reference's
    /// own test replaces `monotonic_ns`.
    pub clock: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
}

impl AgentLoopConfig {
    /// A config for one unbounded run with no tools.
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        model: impl Into<String>,
        system: impl Into<String>,
        messages: Arc<Mutex<Vec<AgentMessage>>>,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            system: system.into(),
            messages,
            tools: Vec::new(),
            prompts: Vec::new(),
            prelude_messages: Vec::new(),
            max_turns: None,
            signal: None,
            session_id: None,
            steering: None,
            follow_up: None,
            before_tool_call: None,
            after_tool_call: None,
            clock: None,
        }
    }
}

/// Run the provider/tool loop, emitting Pi-compatible agent events.
///
/// The loop ends when the model stops asking for tools and the follow-up queue is
/// empty, when a turn fails, or when the turn limit is reached.
pub fn run_agent_loop(config: AgentLoopConfig) -> AgentStream {
    let AgentLoopConfig {
        provider,
        model,
        system,
        messages,
        tools,
        prompts,
        prelude_messages,
        max_turns,
        signal,
        session_id,
        steering,
        follow_up,
        before_tool_call,
        after_tool_call,
        clock,
    } = config;

    let now: Arc<dyn Fn() -> u64 + Send + Sync> = clock.unwrap_or_else(default_clock);

    Box::pin(async_stream::stream! {
        let mut new_messages: Vec<AgentMessage> = prompts.clone();
        if !prompts.is_empty() {
            {
                let mut stored = messages.lock().expect("the transcript lock");
                stored.extend(prompts.iter().cloned());
            }
        }

        yield AgentEvent::AgentStart;
        yield AgentEvent::TurnStart;
        for message in &prelude_messages {
            yield AgentEvent::MessageStart { message: message.clone() };
            yield AgentEvent::MessageEnd { message: message.clone() };
        }
        for prompt in &prompts {
            yield AgentEvent::MessageStart { message: prompt.clone() };
            yield AgentEvent::MessageEnd { message: prompt.clone() };
        }

        if let Some(limit) = max_turns
            && limit < 1
        {
            let error: AgentMessage =
                error_message(&model, "max_turns must be at least 1").into();
            store(&messages, &error);
            new_messages.push(error.clone());
            yield AgentEvent::MessageStart { message: error.clone() };
            yield AgentEvent::MessageEnd { message: error.clone() };
            yield AgentEvent::TurnEnd { message: error, tool_results: Vec::new() };
            yield AgentEvent::AgentEnd { messages: new_messages };
            return;
        }

        let mut tool_by_name: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        for tool in &tools {
            tool_by_name.insert(tool.spec().name.clone(), Arc::clone(tool));
        }

        let mut turn: u32 = 1;
        let mut first_turn = true;
        let mut pending: Vec<AgentMessage> = drain(&steering);

        'outer: loop {
            let mut has_more_tools = true;

            while has_more_tools || !pending.is_empty() {
                if !first_turn {
                    yield AgentEvent::TurnStart;
                }
                first_turn = false;

                for message in std::mem::take(&mut pending) {
                    store(&messages, &message);
                    new_messages.push(message.clone());
                    yield AgentEvent::MessageStart { message: message.clone() };
                    yield AgentEvent::MessageEnd { message };
                }

                if let Some(limit) = max_turns
                    && turn > limit
                {
                    let error: AgentMessage =
                        error_message(&model, &format!("Agent stopped after max_turns={limit}")).into();
                    store(&messages, &error);
                    new_messages.push(error.clone());
                    yield AgentEvent::MessageStart { message: error.clone() };
                    yield AgentEvent::MessageEnd { message: error.clone() };
                    yield AgentEvent::TurnEnd { message: error, tool_results: Vec::new() };
                    yield AgentEvent::AgentEnd { messages: new_messages };
                    return;
                }

                let context = {
                    let stored = messages.lock().expect("the transcript lock");
                    provider_context(&stored)
                };

                let mut started = false;
                let mut assistant: Option<AssistantMessage> = None;
                let mut provider_elapsed_ns: u64 = 0;
                let mut first_output_ns: Option<u64> = None;

                let mut source = provider.stream_response(ModelRequest {
                    model: model.clone(),
                    system: system.clone(),
                    messages: context,
                    tools: tools.iter().map(|tool| tool.spec().clone()).collect(),
                    session_id: session_id.clone(),
                    signal: signal.clone(),
                });

                loop {
                    let wait_started = now();
                    let Some(event) = source.next().await else {
                        break;
                    };
                    provider_elapsed_ns += now().saturating_sub(wait_started);

                    if first_output_ns.is_none() && reports_output(&event) {
                        first_output_ns = Some(provider_elapsed_ns);
                    }

                    match event {
                        AssistantMessageEvent::Start { partial } => {
                            started = true;
                            yield AgentEvent::MessageStart { message: partial.into() };
                        }
                        AssistantMessageEvent::Done { mut message, .. } => {
                            message.timing =
                                Some(response_timing(first_output_ns, provider_elapsed_ns));
                            if !started {
                                yield AgentEvent::MessageStart { message: message.clone().into() };
                            }
                            yield AgentEvent::MessageEnd { message: message.clone().into() };
                            assistant = Some(message);
                        }
                        AssistantMessageEvent::Error { mut error, .. } => {
                            error.timing =
                                Some(response_timing(first_output_ns, provider_elapsed_ns));
                            if !started {
                                yield AgentEvent::MessageStart { message: error.clone().into() };
                            }
                            yield AgentEvent::MessageEnd { message: error.clone().into() };
                            assistant = Some(error);
                        }
                        other => {
                            yield AgentEvent::MessageUpdate {
                                message: other.partial().clone().into(),
                                assistant_message_event: Box::new(other),
                            };
                        }
                    }
                }

                let assistant = match assistant {
                    Some(assistant) => assistant,
                    None => {
                        // Defensive: a provider that ends without a terminal event.
                        let error = error_message(&model, "Provider produced no assistant message");
                        yield AgentEvent::MessageStart { message: error.clone().into() };
                        yield AgentEvent::MessageEnd { message: error.clone().into() };
                        error
                    }
                };

                let assistant: AgentMessage = assistant.into();
                store(&messages, &assistant);
                new_messages.push(assistant.clone());

                let stop_reason = match &assistant {
                    AgentMessage::Assistant(assistant) => assistant.stop_reason,
                    _ => StopReason::Stop,
                };
                if matches!(stop_reason, StopReason::Error | StopReason::Aborted) {
                    yield AgentEvent::TurnEnd { message: assistant, tool_results: Vec::new() };
                    yield AgentEvent::AgentEnd { messages: new_messages };
                    return;
                }

                let calls: Vec<ToolCall> = match &assistant {
                    AgentMessage::Assistant(assistant) => {
                        assistant.tool_calls().into_iter().cloned().collect()
                    }
                    _ => Vec::new(),
                };
                has_more_tools = !calls.is_empty();
                let mut tool_results: Vec<ToolResultMessage> = Vec::new();

                for call in calls {
                    yield AgentEvent::ToolExecutionStart {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        args: call.arguments.clone(),
                    };

                    let mut blocked = false;
                    let mut block_reason: Option<String> = None;
                    if let Some(hook) = &before_tool_call {
                        let (is_blocked, reason) = hook(call.clone()).await;
                        blocked = is_blocked;
                        block_reason = reason;
                    }

                    let (result, is_error) = if blocked {
                        (
                            error_result(
                                block_reason.as_deref().unwrap_or("Tool execution was blocked"),
                            ),
                            true,
                        )
                    } else if signal.as_ref().is_some_and(|signal| signal.is_cancelled()) {
                        (error_result("Operation aborted"), true)
                    } else if let Some(tool) = tool_by_name.get(&call.name) {
                        let updates = ToolUpdates::new();
                        let outcome = tool.execute(&call.id, &call.arguments, &updates).await;

                        // Progress is reported whether the tool succeeded or failed,
                        // matching the reference, which collects it either way.
                        for partial in updates.take() {
                            yield AgentEvent::ToolExecutionUpdate {
                                tool_call_id: call.id.clone(),
                                tool_name: call.name.clone(),
                                args: call.arguments.clone(),
                                partial_result: partial,
                            };
                        }

                        match outcome {
                            Ok(result) => (result, false),
                            Err(error) => (error_result(error.message()), true),
                        }
                    } else {
                        (error_result(&format!("Tool {} not found", call.name)), true)
                    };

                    let (result, is_error) = match &after_tool_call {
                        Some(hook) => hook(call.clone(), result, is_error).await,
                        None => (result, is_error),
                    };

                    yield AgentEvent::ToolExecutionEnd {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        result: result.clone(),
                        is_error,
                    };

                    let message: AgentMessage = ToolResultMessage {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        content: result.content.clone(),
                        details: result.details.clone(),
                        added_tool_names: result.added_tool_names.clone(),
                        is_error,
                        ..ToolResultMessage::default()
                    }
                    .into();

                    if let AgentMessage::ToolResult(result) = &message {
                        tool_results.push(result.clone());
                    }
                    store(&messages, &message);
                    new_messages.push(message.clone());
                    yield AgentEvent::MessageStart { message: message.clone() };
                    yield AgentEvent::MessageEnd { message };
                }

                yield AgentEvent::TurnEnd { message: assistant, tool_results };
                turn += 1;
                pending = drain(&steering);
            }

            let follow_ups = drain(&follow_up);
            if !follow_ups.is_empty() {
                pending = follow_ups;
                continue 'outer;
            }
            break 'outer;
        }

        yield AgentEvent::AgentEnd { messages: new_messages };
    })
}

/// The transcript as the provider should see it.
///
/// A terminal failure is kept in durable history for diagnostics, but an empty
/// failed or aborted turn is not model context and must not poison the request.
fn provider_context(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    let replayable: Vec<AgentMessage> = messages
        .iter()
        .filter(|message| {
            !matches!(
                message,
                AgentMessage::Assistant(assistant)
                    if matches!(assistant.stop_reason, StopReason::Error | StopReason::Aborted)
                        && assistant.content.is_empty()
            )
        })
        .cloned()
        .collect();

    repair_tool_history(&replayable).messages
}

/// Whether an event counts as the first visible output of a response.
fn reports_output(event: &AssistantMessageEvent) -> bool {
    matches!(
        event,
        AssistantMessageEvent::TextDelta { .. }
            | AssistantMessageEvent::ThinkingDelta { .. }
            | AssistantMessageEvent::ToolCallStart { .. }
            | AssistantMessageEvent::ToolCallDelta { .. }
            | AssistantMessageEvent::ToolCallEnd { .. }
    )
}

fn response_timing(first_output_ns: Option<u64>, total_ns: u64) -> ResponseTiming {
    ResponseTiming {
        time_to_first_output_ms: first_output_ns.map(|elapsed| elapsed / 1_000_000),
        total_duration_ms: total_ns / 1_000_000,
    }
}

/// The result recorded when a tool could not produce one.
fn error_result(message: &str) -> AgentToolResult {
    AgentToolResult {
        content: vec![
            TextContent {
                text: message.to_string(),
                text_signature: None,
            }
            .into(),
        ],
        details: Some(json!({})),
        ..AgentToolResult::default()
    }
}

/// The assistant message that reports the loop could not proceed.
fn error_message(model: &str, message: &str) -> AssistantMessage {
    AssistantMessage {
        model: model.to_string(),
        content: Vec::new(),
        stop_reason: StopReason::Error,
        error_message: Some(message.to_string()),
        ..AssistantMessage::default()
    }
}

fn store(messages: &Arc<Mutex<Vec<AgentMessage>>>, message: &AgentMessage) {
    messages
        .lock()
        .expect("the transcript lock")
        .push(message.clone());
}

fn drain(source: &Option<Arc<dyn Fn() -> Vec<AgentMessage> + Send + Sync>>) -> Vec<AgentMessage> {
    source.as_ref().map(|pull| pull()).unwrap_or_default()
}

fn default_clock() -> Arc<dyn Fn() -> u64 + Send + Sync> {
    Arc::new(monotonic_ns)
}

/// A monotonic nanosecond counter, mirroring the reference's `monotonic_ns`.
fn monotonic_ns() -> u64 {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    ANCHOR.get_or_init(Instant::now).elapsed().as_nanos() as u64
}
