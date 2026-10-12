//! The events the portable agent layer emits to any frontend.

use serde::{Deserialize, Serialize};

use crate::json::JsonObject;
use crate::message::{AgentMessage, ToolResultMessage};
use crate::provider_event::AssistantMessageEvent;
use crate::tool::AgentToolResult;

/// An event emitted by the agent loop.
///
/// Like the provider events, and for the same serde limitation, unknown fields
/// are tolerated here. See [`AssistantMessageEvent`](crate::provider_event::AssistantMessageEvent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum AgentEvent {
    /// The agent run started.
    #[serde(rename = "agent_start")]
    AgentStart,
    /// The agent run finished, carrying the whole transcript.
    #[serde(rename = "agent_end")]
    AgentEnd {
        /// Every message in the run.
        messages: Vec<AgentMessage>,
    },
    /// A turn started.
    #[serde(rename = "turn_start")]
    TurnStart,
    /// A turn finished.
    #[serde(rename = "turn_end")]
    TurnEnd {
        /// The assistant message that produced the turn.
        message: AgentMessage,
        /// The tool results collected in the turn.
        tool_results: Vec<ToolResultMessage>,
    },
    /// A message was appended to the transcript.
    #[serde(rename = "message_start")]
    MessageStart {
        /// The message.
        message: AgentMessage,
    },
    /// A streamed fragment of the current message arrived.
    #[serde(rename = "message_update")]
    MessageUpdate {
        /// The message so far.
        message: AgentMessage,
        /// The provider event that caused the update.
        ///
        /// Boxed because this event streams once per token: without the box every
        /// yielded event would be roughly half a kilobyte, moved through a channel
        /// on each fragment.
        assistant_message_event: Box<AssistantMessageEvent>,
    },
    /// A message was completed.
    #[serde(rename = "message_end")]
    MessageEnd {
        /// The completed message.
        message: AgentMessage,
    },
    /// A tool call started executing.
    #[serde(rename = "tool_execution_start")]
    ToolExecutionStart {
        /// The id of the tool call being executed.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
        /// The arguments it was called with.
        args: JsonObject,
    },
    /// A tool reported partial progress.
    #[serde(rename = "tool_execution_update")]
    ToolExecutionUpdate {
        /// The id of the tool call being executed.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
        /// The arguments it was called with.
        args: JsonObject,
        /// The partial result.
        partial_result: AgentToolResult,
    },
    /// A tool call finished.
    #[serde(rename = "tool_execution_end")]
    ToolExecutionEnd {
        /// The id of the tool call that finished.
        tool_call_id: String,
        /// The tool name.
        tool_name: String,
        /// The final result.
        result: AgentToolResult,
        /// Whether the call failed.
        is_error: bool,
    },
}

impl AgentEvent {
    /// The wire `type` of the event.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AgentStart => "agent_start",
            Self::AgentEnd { .. } => "agent_end",
            Self::TurnStart => "turn_start",
            Self::TurnEnd { .. } => "turn_end",
            Self::MessageStart { .. } => "message_start",
            Self::MessageUpdate { .. } => "message_update",
            Self::MessageEnd { .. } => "message_end",
            Self::ToolExecutionStart { .. } => "tool_execution_start",
            Self::ToolExecutionUpdate { .. } => "tool_execution_update",
            Self::ToolExecutionEnd { .. } => "tool_execution_end",
        }
    }
}
