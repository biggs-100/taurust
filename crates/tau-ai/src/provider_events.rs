//! The adapter-level provider lifecycle events.
//!
//! These surround a request — it started, it is being retried, it produced a
//! delta, it ended. They are a different family from the canonical assistant
//! stream in `tau_protocol::provider_event`, which is what the agent loop and
//! the frontends consume.
//!
//! Ported from tau's `tau_ai/_provider_events.py`.

use serde::{Deserialize, Serialize};

use tau_protocol::JsonValue;
use tau_protocol::content::ToolCall;
use tau_protocol::message::AssistantMessage;

/// An event a provider adapter emits about the request it is making.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum ProviderEvent {
    /// The adapter started a request.
    #[serde(rename = "response_start")]
    ResponseStart {
        /// The model being requested.
        model: String,
        /// The provider actually serving the request, when it differs.
        #[serde(default)]
        response_provider: Option<String>,
    },
    /// The adapter is retrying a transient failure.
    #[serde(rename = "retry")]
    Retry {
        /// The attempt number that is about to run.
        attempt: u32,
        /// How many attempts are allowed in total.
        max_attempts: u32,
        /// How long the adapter will wait before the next attempt.
        delay_seconds: f64,
        /// The human-readable progress message.
        message: String,
        /// Structured detail for frontends.
        #[serde(default)]
        data: Option<JsonValue>,
    },
    /// A streamed text fragment.
    #[serde(rename = "text_delta")]
    TextDelta {
        /// The fragment.
        delta: String,
    },
    /// A streamed reasoning fragment.
    #[serde(rename = "thinking_delta")]
    ThinkingDelta {
        /// The fragment.
        delta: String,
    },
    /// A complete tool call requested by the model.
    #[serde(rename = "tool_call")]
    ToolCall {
        /// The tool call.
        tool_call: ToolCall,
    },
    /// The adapter finished a request.
    #[serde(rename = "response_end")]
    ResponseEnd {
        /// The final assistant message.
        ///
        /// Boxed because it dwarfs every other variant, and these events are
        /// moved through channels.
        message: Box<AssistantMessage>,
        /// The provider's finish reason, when it gave one.
        #[serde(default)]
        finish_reason: Option<String>,
    },
    /// The adapter failed a request.
    #[serde(rename = "error")]
    Error {
        /// The failure message.
        message: String,
        /// Structured detail for frontends.
        #[serde(default)]
        data: Option<JsonValue>,
        /// The provider involved, when it is known.
        #[serde(default)]
        response_provider: Option<String>,
    },
}

impl ProviderEvent {
    /// The wire `type` of the event.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ResponseStart { .. } => "response_start",
            Self::Retry { .. } => "retry",
            Self::TextDelta { .. } => "text_delta",
            Self::ThinkingDelta { .. } => "thinking_delta",
            Self::ToolCall { .. } => "tool_call",
            Self::ResponseEnd { .. } => "response_end",
            Self::Error { .. } => "error",
        }
    }
}
