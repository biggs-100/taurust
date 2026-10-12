//! `tau-protocol` — the provider-neutral vocabulary shared by every layer.
//!
//! Holds the content blocks, transcript messages, tool definitions, streaming
//! events and the provider trait. Nothing here knows about a terminal, a local
//! configuration path or rendering.
//!
//! This crate exists because in tau the provider contract is owned by the agent
//! layer while the provider adapters live in `tau_ai`, so `tau_ai` imports
//! `tau_agent`. Rust has no reason to reproduce that inversion: the shared
//! vocabulary is a leaf that both sides depend on.

#![forbid(unsafe_code)]

pub mod content;
pub mod json;
pub mod message;

pub use content::{
    AssistantContent, ImageContent, TextContent, ThinkingContent, ToolCall, ToolResultContent,
    UserContent, UserContentBlock,
};
pub use json::{JsonObject, JsonValue};
pub use message::{
    AgentMessage, AssistantDiagnosticError, AssistantMessage, AssistantMessageDiagnostic,
    BashExecutionMessage, BranchSummaryMessage, CompactionSummaryMessage, CustomMessage,
    DiagnosticCode, ResponseTiming, StopReason, ToolResultMessage, Usage, UsageCost, UserMessage,
};
