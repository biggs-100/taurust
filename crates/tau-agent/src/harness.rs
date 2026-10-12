//! The stateful agent harness the frontends drive.
//!
//! Ported from tau's `tau_agent/harness.py`. It owns the transcript and the two
//! message queues, rejects a run that would overlap another, and repairs tool
//! calls that never returned — both when a run starts and again when a run is
//! cancelled.
//!
//! **Deviation from the reference, deliberate.** The reference's listeners may be
//! async and the harness awaits them. Rust cannot await inside `Drop`, and the
//! harness has to push repairs to subscribers when a run is abandoned, so
//! listeners are synchronous here. A listener that needs to do async work can
//! forward the event to a channel.
//!
//! The state lives behind an `Arc` rather than being borrowed, because a caller
//! steers a run *while* consuming its events: `follow_up` is called from inside
//! the stream, which is impossible if the stream borrows the harness mutably.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;

use tau_protocol::AgentEvent;
use tau_protocol::content::TextContent;
use tau_protocol::message::{AgentMessage, ToolResultMessage, UserMessage};
use tau_protocol::provider::{CancellationToken, ManualCancellation, ModelProvider};

use crate::agent_loop::{
    AfterToolCall, AgentLoopConfig, AgentStream, BeforeToolCall, run_agent_loop,
};
use crate::tool_history::INTERRUPTED_TOOL_RESULT;
use crate::tools::Tool;

/// How many queued messages are drained into a turn at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueMode {
    /// Inject one queued message at a time.
    #[default]
    OneAtATime,
    /// Inject every queued message together.
    All,
}

/// The messages waiting to be injected into a run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueuedMessages {
    /// Messages injected at the start of the next turn.
    pub steering: Vec<AgentMessage>,
    /// Messages that continue a run when it would otherwise end.
    pub follow_up: Vec<AgentMessage>,
}

impl QueuedMessages {
    /// How many messages are queued in total.
    pub fn count(&self) -> usize {
        self.steering.len() + self.follow_up.len()
    }
}

/// Why a run could not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessError {
    /// A run is already in progress.
    AlreadyRunning,
}

impl fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => formatter.write_str(
                "AgentHarness is already running; use steer() or follow_up() to queue messages.",
            ),
        }
    }
}

impl std::error::Error for HarnessError {}

/// A listener notified of every event the harness emits.
///
/// It must not panic: one that does while a run is being torn down is ignored
/// rather than allowed to abort the process.
pub type EventListener = Arc<dyn Fn(AgentEvent) + Send + Sync>;

/// Everything a harness needs to run.
#[derive(Clone)]
pub struct AgentHarnessConfig {
    /// The model provider.
    pub provider: Arc<dyn ModelProvider>,
    /// The model name.
    pub model: String,
    /// The system prompt.
    pub system: String,
    /// The tools the model may call.
    pub tools: Vec<Arc<dyn Tool>>,
    /// The maximum number of turns, when the run is bounded.
    pub max_turns: Option<u32>,
    /// How many queued messages are drained at once.
    pub queue_mode: QueueMode,
    /// A session id, used by providers for request routing or cache affinity.
    pub session_id: Option<String>,
    /// Consulted before every tool call.
    pub before_tool_call: Option<BeforeToolCall>,
    /// Consulted after every tool call.
    pub after_tool_call: Option<AfterToolCall>,
}

impl AgentHarnessConfig {
    /// A config with no tools and the default queue behaviour.
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        model: impl Into<String>,
        system: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            system: system.into(),
            tools: Vec::new(),
            max_turns: None,
            queue_mode: QueueMode::default(),
            session_id: None,
            before_tool_call: None,
            after_tool_call: None,
        }
    }
}

struct RegisteredListener {
    id: u64,
    listener: EventListener,
}

struct HarnessState {
    config: AgentHarnessConfig,
    messages: Arc<Mutex<Vec<AgentMessage>>>,
    steering: Mutex<VecDeque<AgentMessage>>,
    follow_up: Mutex<VecDeque<AgentMessage>>,
    signal: Mutex<Option<Arc<ManualCancellation>>>,
    running: AtomicBool,
    next_listener_id: AtomicU64,
    listeners: Mutex<Vec<RegisteredListener>>,
}

impl HarnessState {
    fn drain(&self, queue: &Mutex<VecDeque<AgentMessage>>) -> Vec<AgentMessage> {
        let mut queued = queue.lock().expect("a harness queue lock");
        if queued.is_empty() {
            return Vec::new();
        }
        if self.config.queue_mode == QueueMode::All {
            queued.drain(..).collect()
        } else {
            queued.pop_front().into_iter().collect()
        }
    }

    fn drain_steering(&self) -> Vec<AgentMessage> {
        self.drain(&self.steering)
    }

    fn drain_follow_up(&self) -> Vec<AgentMessage> {
        self.drain(&self.follow_up)
    }

    fn notify(&self, event: &AgentEvent) {
        for listener in self.listener_snapshot() {
            listener(event.clone());
        }
    }

    /// Notify while a run is being torn down, where a panicking listener must not
    /// abort the process during an unwind.
    fn notify_ignoring_panics(&self, event: &AgentEvent) {
        for listener in self.listener_snapshot() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener(event.clone());
            }));
        }
    }

    fn listener_snapshot(&self) -> Vec<EventListener> {
        self.listeners
            .lock()
            .expect("the listener lock")
            .iter()
            .map(|registered| Arc::clone(&registered.listener))
            .collect()
    }

    /// Append a deterministic error result for every tool call that never
    /// returned, and report what was appended.
    ///
    /// A result whose id is already present counts as returned, and a call whose
    /// id is repeated only gets one synthesized result per pass, so this is safe
    /// to run more than once.
    fn append_interrupted_tool_results(&self) -> Vec<AgentMessage> {
        let mut stored = self.messages.lock().expect("the transcript lock");
        let snapshot = stored.clone();

        let mut returned: HashSet<String> = snapshot
            .iter()
            .filter_map(|message| match message {
                AgentMessage::ToolResult(result) => Some(result.tool_call_id.clone()),
                _ => None,
            })
            .collect();

        let mut added = Vec::new();
        for message in snapshot {
            let AgentMessage::Assistant(assistant) = message else {
                continue;
            };
            for call in assistant.tool_calls() {
                if !returned.insert(call.id.clone()) {
                    continue;
                }
                let result: AgentMessage = ToolResultMessage {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    content: vec![
                        TextContent {
                            text: INTERRUPTED_TOOL_RESULT.to_string(),
                            text_signature: None,
                        }
                        .into(),
                    ],
                    is_error: true,
                    ..ToolResultMessage::default()
                }
                .into();
                stored.push(result.clone());
                added.push(result);
            }
        }

        added
    }
}

/// Clears the run flag and repairs the transcript when a run is abandoned.
struct RunGuard {
    state: Arc<HarnessState>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let cancelled = self
            .state
            .signal
            .lock()
            .expect("the signal lock")
            .as_ref()
            .is_some_and(|signal| signal.is_cancelled());

        if cancelled {
            // The consumer is usually gone by now, so the repairs are pushed to
            // subscribers — which is what persists them — rather than yielded.
            for message in self.state.append_interrupted_tool_results() {
                self.state
                    .notify_ignoring_panics(&AgentEvent::MessageStart {
                        message: message.clone(),
                    });
                self.state
                    .notify_ignoring_panics(&AgentEvent::MessageEnd { message });
            }
        }

        *self.state.signal.lock().expect("the signal lock") = None;
        self.state.running.store(false, Ordering::SeqCst);
    }
}

/// A live subscription. Dropping it stops the notifications.
pub struct Subscription {
    state: Arc<HarnessState>,
    id: u64,
    active: AtomicBool,
}

impl Subscription {
    /// Stop receiving events, exactly as dropping the subscription does.
    pub fn unsubscribe(self) {
        drop(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if self.active.swap(false, Ordering::SeqCst) {
            self.state
                .listeners
                .lock()
                .expect("the listener lock")
                .retain(|registered| registered.id != self.id);
        }
    }
}

/// A reusable stateful agent brain, independent of coding and UI policy.
#[derive(Clone)]
pub struct AgentHarness {
    state: Arc<HarnessState>,
}

impl AgentHarness {
    /// A harness with an empty transcript.
    pub fn new(config: AgentHarnessConfig) -> Self {
        Self::with_messages(config, Vec::new())
    }

    /// A harness that continues an existing transcript.
    pub fn with_messages(config: AgentHarnessConfig, messages: Vec<AgentMessage>) -> Self {
        Self {
            state: Arc::new(HarnessState {
                config,
                messages: Arc::new(Mutex::new(messages)),
                steering: Mutex::new(VecDeque::new()),
                follow_up: Mutex::new(VecDeque::new()),
                signal: Mutex::new(None),
                running: AtomicBool::new(false),
                next_listener_id: AtomicU64::new(1),
                listeners: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The transcript so far.
    pub fn messages(&self) -> Vec<AgentMessage> {
        self.state
            .messages
            .lock()
            .expect("the transcript lock")
            .clone()
    }

    /// The configuration this harness runs with.
    pub fn config(&self) -> &AgentHarnessConfig {
        &self.state.config
    }

    /// Whether a run is in progress.
    pub fn is_running(&self) -> bool {
        self.state.running.load(Ordering::SeqCst)
    }

    /// The messages waiting to be injected.
    pub fn queued_messages(&self) -> QueuedMessages {
        QueuedMessages {
            steering: self
                .state
                .steering
                .lock()
                .expect("the steering lock")
                .iter()
                .cloned()
                .collect(),
            follow_up: self
                .state
                .follow_up
                .lock()
                .expect("the follow-up lock")
                .iter()
                .cloned()
                .collect(),
        }
    }

    /// How many messages are queued.
    pub fn pending_message_count(&self) -> usize {
        self.queued_messages().count()
    }

    /// Whether anything is queued.
    pub fn has_queued_messages(&self) -> bool {
        self.pending_message_count() > 0
    }

    /// Append a message to the transcript without running the agent.
    pub fn append_message(&self, message: AgentMessage) {
        self.state
            .messages
            .lock()
            .expect("the transcript lock")
            .push(message);
    }

    /// Replace the transcript.
    pub fn replace_messages(&self, messages: Vec<AgentMessage>) {
        *self.state.messages.lock().expect("the transcript lock") = messages;
    }

    /// Subscribe to every event the harness emits.
    pub fn subscribe(&self, listener: EventListener) -> Subscription {
        let id = self.state.next_listener_id.fetch_add(1, Ordering::SeqCst);
        self.state
            .listeners
            .lock()
            .expect("the listener lock")
            .push(RegisteredListener { id, listener });

        Subscription {
            state: Arc::clone(&self.state),
            id,
            active: AtomicBool::new(true),
        }
    }

    /// Cancel the run in progress, if there is one.
    pub fn cancel(&self) {
        if let Some(signal) = self.state.signal.lock().expect("the signal lock").as_ref() {
            signal.cancel();
        }
    }

    /// Queue a message for the start of the next turn.
    pub fn steer(&self, content: &str) -> QueuedMessages {
        self.steer_message(user_message(content))
    }

    /// Queue a message for the start of the next turn.
    pub fn steer_message(&self, message: AgentMessage) -> QueuedMessages {
        self.state
            .steering
            .lock()
            .expect("the steering lock")
            .push_back(message);
        self.queued_messages()
    }

    /// Queue a message that continues the run when it would otherwise end.
    pub fn follow_up(&self, content: &str) -> QueuedMessages {
        self.follow_up_message(user_message(content))
    }

    /// Queue a message that continues the run when it would otherwise end.
    pub fn follow_up_message(&self, message: AgentMessage) -> QueuedMessages {
        self.state
            .follow_up
            .lock()
            .expect("the follow-up lock")
            .push_back(message);
        self.queued_messages()
    }

    /// Drop everything queued, returning what was there.
    pub fn clear_queues(&self) -> QueuedMessages {
        let snapshot = self.queued_messages();
        self.state
            .steering
            .lock()
            .expect("the steering lock")
            .clear();
        self.state
            .follow_up
            .lock()
            .expect("the follow-up lock")
            .clear();
        snapshot
    }

    /// Take the most recently queued follow-up.
    pub fn pop_latest_follow_up(&self) -> Option<AgentMessage> {
        self.state
            .follow_up
            .lock()
            .expect("the follow-up lock")
            .pop_back()
    }

    /// Take the most recently queued steering message.
    pub fn pop_latest_steering(&self) -> Option<AgentMessage> {
        self.state
            .steering
            .lock()
            .expect("the steering lock")
            .pop_back()
    }

    /// Run the agent with one user message.
    pub fn prompt(&self, content: &str) -> Result<AgentStream, HarnessError> {
        self.begin(vec![user_message(content)])
    }

    /// Run the agent with one message of any kind.
    pub fn prompt_message(&self, message: AgentMessage) -> Result<AgentStream, HarnessError> {
        self.begin(vec![message])
    }

    /// Continue the run without adding a prompt.
    pub fn continue_(&self) -> Result<AgentStream, HarnessError> {
        self.begin(Vec::new())
    }

    /// Repair the transcript now, returning how many results were appended.
    pub fn append_interrupted_tool_results(&self) -> usize {
        self.state.append_interrupted_tool_results().len()
    }

    fn begin(&self, prompts: Vec<AgentMessage>) -> Result<AgentStream, HarnessError> {
        if self.state.running.swap(true, Ordering::SeqCst) {
            return Err(HarnessError::AlreadyRunning);
        }

        // Repair here, not in the callers, so the synthetic results flow through
        // events and reach subscribers as well as the consumer.
        let prelude = self.state.append_interrupted_tool_results();

        let signal = Arc::new(ManualCancellation::default());
        *self.state.signal.lock().expect("the signal lock") = Some(Arc::clone(&signal));

        let state = Arc::clone(&self.state);
        let steering_state = Arc::clone(&self.state);
        let follow_up_state = Arc::clone(&self.state);

        let mut inner = run_agent_loop(AgentLoopConfig {
            provider: Arc::clone(&state.config.provider),
            model: state.config.model.clone(),
            system: state.config.system.clone(),
            messages: Arc::clone(&state.messages),
            tools: state.config.tools.clone(),
            prompts,
            prelude_messages: prelude,
            max_turns: state.config.max_turns,
            signal: Some(Arc::clone(&signal) as Arc<dyn CancellationToken>),
            session_id: state.config.session_id.clone(),
            steering: Some(Arc::new(move || steering_state.drain_steering())),
            follow_up: Some(Arc::new(move || follow_up_state.drain_follow_up())),
            before_tool_call: state.config.before_tool_call.clone(),
            after_tool_call: state.config.after_tool_call.clone(),
            clock: None,
        });

        Ok(Box::pin(async_stream::stream! {
            let _guard = RunGuard { state: Arc::clone(&state) };
            while let Some(event) = inner.next().await {
                state.notify(&event);
                yield event;
            }
        }))
    }
}

fn user_message(content: &str) -> AgentMessage {
    UserMessage {
        content: content.into(),
        ..UserMessage::default()
    }
    .into()
}
