//! The canonical assistant streaming events.
//!
//! Provider adapters translate their wire format into these; every frontend and
//! the agent loop only ever see this vocabulary.
//!
//! Each event carries a `partial` snapshot of the assistant message as it stood
//! when the event was produced, mirroring Pi.

use serde::{Deserialize, Serialize};

use crate::content::ToolCall;
use crate::message::AssistantMessage;

/// Why a completed assistant response finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DoneReason {
    /// The model finished its answer.
    #[default]
    #[serde(rename = "stop")]
    Stop,
    /// The model hit the output token limit.
    #[serde(rename = "length")]
    Length,
    /// The model requested tool calls.
    #[serde(rename = "toolUse")]
    ToolUse,
}

/// Why an assistant response failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorReason {
    /// The request was cancelled.
    #[serde(rename = "aborted")]
    Aborted,
    /// The request failed.
    #[serde(rename = "error")]
    Error,
}

/// A provider-neutral assistant streaming event.
///
/// Unknown fields are tolerated on deserialization. Serde silently ignores
/// `deny_unknown_fields` on an internally tagged enum, and tolerating them is
/// the friendlier behaviour for a transient wire event anyway: a provider that
/// starts sending a new field must not break the stream. Messages, which are
/// persisted, stay strict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantMessageEvent {
    /// The assistant response started.
    #[serde(rename = "start")]
    Start {
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A text block started.
    #[serde(rename = "text_start")]
    TextStart {
        /// The index of the block inside the message.
        content_index: u32,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A text fragment arrived.
    #[serde(rename = "text_delta")]
    TextDelta {
        /// The index of the block inside the message.
        content_index: u32,
        /// The fragment.
        delta: String,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A text block finished.
    #[serde(rename = "text_end")]
    TextEnd {
        /// The index of the block inside the message.
        content_index: u32,
        /// The finished text.
        content: String,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A reasoning block started.
    #[serde(rename = "thinking_start")]
    ThinkingStart {
        /// The index of the block inside the message.
        content_index: u32,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A reasoning fragment arrived.
    #[serde(rename = "thinking_delta")]
    ThinkingDelta {
        /// The index of the block inside the message.
        content_index: u32,
        /// The fragment.
        delta: String,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A reasoning block finished.
    #[serde(rename = "thinking_end")]
    ThinkingEnd {
        /// The index of the block inside the message.
        content_index: u32,
        /// The finished reasoning text.
        content: String,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A tool call block started.
    #[serde(rename = "toolcall_start")]
    ToolCallStart {
        /// The index of the block inside the message.
        content_index: u32,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A tool call argument fragment arrived.
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta {
        /// The index of the block inside the message.
        content_index: u32,
        /// The fragment.
        delta: String,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// A tool call finished.
    #[serde(rename = "toolcall_end")]
    ToolCallEnd {
        /// The index of the block inside the message.
        content_index: u32,
        /// The completed tool call.
        tool_call: ToolCall,
        /// The assistant message so far.
        partial: AssistantMessage,
    },
    /// The response completed.
    #[serde(rename = "done")]
    Done {
        /// Why it completed.
        reason: DoneReason,
        /// The final assistant message.
        message: AssistantMessage,
    },
    /// The response failed.
    #[serde(rename = "error")]
    Error {
        /// Why it failed.
        reason: ErrorReason,
        /// The final assistant message, carrying the failure.
        error: AssistantMessage,
    },
}

impl AssistantMessageEvent {
    /// The wire `type` of the event.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::TextStart { .. } => "text_start",
            Self::TextDelta { .. } => "text_delta",
            Self::TextEnd { .. } => "text_end",
            Self::ThinkingStart { .. } => "thinking_start",
            Self::ThinkingDelta { .. } => "thinking_delta",
            Self::ThinkingEnd { .. } => "thinking_end",
            Self::ToolCallStart { .. } => "toolcall_start",
            Self::ToolCallDelta { .. } => "toolcall_delta",
            Self::ToolCallEnd { .. } => "toolcall_end",
            Self::Done { .. } => "done",
            Self::Error { .. } => "error",
        }
    }

    /// The assistant message this event reports on.
    pub fn partial(&self) -> &AssistantMessage {
        match self {
            Self::Start { partial }
            | Self::TextStart { partial, .. }
            | Self::TextDelta { partial, .. }
            | Self::TextEnd { partial, .. }
            | Self::ThinkingStart { partial, .. }
            | Self::ThinkingDelta { partial, .. }
            | Self::ThinkingEnd { partial, .. }
            | Self::ToolCallStart { partial, .. }
            | Self::ToolCallDelta { partial, .. }
            | Self::ToolCallEnd { partial, .. } => partial,
            Self::Done { message, .. } => message,
            Self::Error { error, .. } => error,
        }
    }
}
