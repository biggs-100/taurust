//! The provider contract every model adapter implements.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_core::Stream;

use crate::message::AgentMessage;
use crate::provider_event::AssistantMessageEvent;
use crate::tool::ToolSpec;

/// A boxed stream of assistant events.
pub type ProviderStream = Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>>;

/// Reports whether the current stream should stop.
pub trait CancellationToken: Send + Sync {
    /// Return whether the stream should stop.
    fn is_cancelled(&self) -> bool;
}

/// A token that is never cancelled.
#[derive(Debug, Default, Clone, Copy)]
pub struct NeverCancelled;

impl CancellationToken for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// A token a caller cancels by hand, for tests and for frontends.
#[derive(Debug, Default)]
pub struct ManualCancellation {
    cancelled: AtomicBool,
}

impl ManualCancellation {
    /// Mark the stream as cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl CancellationToken for ManualCancellation {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// One request for a model response.
#[derive(Clone)]
pub struct ModelRequest {
    /// The model name.
    pub model: String,
    /// The system prompt.
    pub system: String,
    /// The conversation so far.
    pub messages: Vec<AgentMessage>,
    /// The tools the model may call.
    pub tools: Vec<ToolSpec>,
    /// A session id, used by providers for request routing or cache affinity.
    pub session_id: Option<String>,
    /// A token the caller can cancel the request with.
    pub signal: Option<Arc<dyn CancellationToken>>,
}

impl fmt::Debug for ModelRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModelRequest")
            .field("model", &self.model)
            .field("system", &self.system)
            .field("messages", &self.messages)
            .field("tools", &self.tools)
            .field("session_id", &self.session_id)
            .field(
                "cancelled",
                &self.signal.as_ref().map(|signal| signal.is_cancelled()),
            )
            .finish()
    }
}

impl ModelRequest {
    /// A request for a model with a system prompt.
    pub fn new(model: impl Into<String>, system: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: system.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            session_id: None,
            signal: None,
        }
    }

    /// Set the conversation.
    pub fn with_messages(mut self, messages: Vec<AgentMessage>) -> Self {
        self.messages = messages;
        self
    }

    /// Set the tools the model may call.
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    /// Set the session id.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set the cancellation token.
    pub fn with_signal(mut self, signal: Arc<dyn CancellationToken>) -> Self {
        self.signal = Some(signal);
        self
    }
}

/// The provider-neutral model stream interface.
pub trait ModelProvider: Send + Sync {
    /// Stream one model response.
    fn stream_response(&self, request: ModelRequest) -> ProviderStream;
}
