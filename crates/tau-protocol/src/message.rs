//! Pi-compatible transcript messages and their usage and timing records.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::content::{AssistantContent, ToolCall, ToolResultContent, UserContent};
use crate::json::JsonValue;

/// The current Unix timestamp in milliseconds.
pub fn current_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn unknown() -> String {
    "unknown".to_string()
}

/// Billed cost of one response, in US dollars.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct UsageCost {
    /// Input token cost.
    pub input: f64,
    /// Output token cost.
    pub output: f64,
    /// Cache read cost.
    pub cache_read: f64,
    /// Cache write cost.
    pub cache_write: f64,
    /// Total cost.
    pub total: f64,
}

/// Provider-reported token usage for one assistant response.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Usage {
    /// Input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Tokens read from the provider cache.
    pub cache_read: u64,
    /// Tokens written to the provider cache.
    pub cache_write: u64,
    /// Tokens written to the one-hour cache tier, when the provider reports it.
    #[serde(rename = "cacheWrite1H")]
    pub cache_write_1h: Option<u64>,
    /// Reasoning tokens, when the provider reports them separately.
    pub reasoning: Option<u64>,
    /// Total tokens.
    pub total_tokens: u64,
    /// Billed cost.
    pub cost: UsageCost,
}

impl Usage {
    /// The field-wise total of one or more provider requests.
    ///
    /// An optional field totals to `None` only when no request reported it, so
    /// billing never invents a zero for a tier the provider stayed silent about.
    pub fn total(usages: impl IntoIterator<Item = Usage>) -> Usage {
        let items: Vec<Usage> = usages.into_iter().collect();

        fn optional_total(items: &[Usage], pick: fn(&Usage) -> Option<u64>) -> Option<u64> {
            if items.iter().any(|item| pick(item).is_some()) {
                Some(items.iter().filter_map(pick).sum())
            } else {
                None
            }
        }

        Usage {
            input: items.iter().map(|item| item.input).sum(),
            output: items.iter().map(|item| item.output).sum(),
            cache_read: items.iter().map(|item| item.cache_read).sum(),
            cache_write: items.iter().map(|item| item.cache_write).sum(),
            cache_write_1h: optional_total(&items, |item| item.cache_write_1h),
            reasoning: optional_total(&items, |item| item.reasoning),
            total_tokens: items.iter().map(|item| item.total_tokens).sum(),
            cost: UsageCost {
                input: items.iter().map(|item| item.cost.input).sum(),
                output: items.iter().map(|item| item.cost.output).sum(),
                cache_read: items.iter().map(|item| item.cost.cache_read).sum(),
                cache_write: items.iter().map(|item| item.cost.cache_write).sum(),
                total: items.iter().map(|item| item.cost.total).sum(),
            },
        }
    }
}

/// Monotonic request durations for one assistant response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ResponseTiming {
    /// Milliseconds until the first visible output, when one was observed.
    pub time_to_first_output_ms: Option<u64>,
    /// Total request duration in milliseconds.
    pub total_duration_ms: u64,
}

/// Why an assistant response stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StopReason {
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
    /// The request failed.
    #[serde(rename = "error")]
    Error,
    /// The request was cancelled.
    #[serde(rename = "aborted")]
    Aborted,
}

/// The `code` of a provider diagnostic: a name or a number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiagnosticCode {
    /// A symbolic code, for example `invalid_api_key`.
    Text(String),
    /// A numeric code.
    Number(i64),
}

/// The error carried by a provider diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantDiagnosticError {
    /// The provider's error name, when it gave one.
    #[serde(default)]
    pub name: Option<String>,
    /// The human-readable message.
    pub message: String,
    /// A stack trace, when the provider sent one.
    #[serde(default)]
    pub stack: Option<String>,
    /// A provider error code, when it gave one.
    #[serde(default)]
    pub code: Option<DiagnosticCode>,
}

/// A provider diagnostic attached to an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantMessageDiagnostic {
    /// The diagnostic kind.
    #[serde(rename = "type")]
    pub kind: String,
    /// When the diagnostic was recorded.
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
    /// The error it reports, when it reports one.
    #[serde(default)]
    pub error: Option<AssistantDiagnosticError>,
    /// Structured provider details.
    #[serde(default)]
    pub details: Option<JsonValue>,
}

/// The `role` marker of a user message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UserRole {
    /// The only valid value: `"user"`.
    #[default]
    #[serde(rename = "user")]
    User,
}

/// The `role` marker of an assistant message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AssistantRole {
    /// The only valid value: `"assistant"`.
    #[default]
    #[serde(rename = "assistant")]
    Assistant,
}

/// The `role` marker of a tool result message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolResultRole {
    /// The only valid value: `"toolResult"`.
    #[default]
    #[serde(rename = "toolResult")]
    ToolResult,
}

/// The `role` marker of a shell execution message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BashExecutionRole {
    /// The only valid value: `"bashExecution"`.
    #[default]
    #[serde(rename = "bashExecution")]
    BashExecution,
}

/// The `role` marker of a custom message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CustomRole {
    /// The only valid value: `"custom"`.
    #[default]
    #[serde(rename = "custom")]
    Custom,
}

/// The `role` marker of a branch summary message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BranchSummaryRole {
    /// The only valid value: `"branchSummary"`.
    #[default]
    #[serde(rename = "branchSummary")]
    BranchSummary,
}

/// The `role` marker of a compaction summary message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompactionSummaryRole {
    /// The only valid value: `"compactionSummary"`.
    #[default]
    #[serde(rename = "compactionSummary")]
    CompactionSummary,
}

/// Text the user sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserMessage {
    /// Always `user`.
    #[serde(default)]
    pub role: UserRole,
    /// The input: a bare string, or blocks when an image is attached.
    pub content: UserContent,
    /// When the message was created.
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

impl Default for UserMessage {
    fn default() -> Self {
        Self {
            role: UserRole::User,
            content: UserContent::default(),
            timestamp: current_timestamp_ms(),
        }
    }
}

impl UserMessage {
    /// The visible text of the message.
    pub fn text(&self) -> String {
        self.content.text()
    }
}

/// A Pi-compatible assistant message with ordered content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantMessage {
    /// Always `assistant`.
    #[serde(default)]
    pub role: AssistantRole,
    /// The ordered text, reasoning and tool call blocks.
    #[serde(default)]
    pub content: Vec<AssistantContent>,
    /// The provider API dialect that produced the message.
    #[serde(default = "unknown")]
    pub api: String,
    /// The configured provider name.
    #[serde(default = "unknown")]
    pub provider: String,
    /// The configured model name.
    #[serde(default = "unknown")]
    pub model: String,
    /// The model the provider actually served, when it reported one.
    #[serde(default)]
    pub response_model: Option<String>,
    /// The provider that actually served the request, when it differed.
    #[serde(default)]
    pub response_provider: Option<String>,
    /// The provider's response id, when it gave one.
    #[serde(default)]
    pub response_id: Option<String>,
    /// Provider diagnostics recorded for this response.
    #[serde(default)]
    pub diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    /// Token usage.
    #[serde(default)]
    pub usage: Usage,
    /// Request timings.
    #[serde(default)]
    pub timing: Option<ResponseTiming>,
    /// Why the response stopped.
    #[serde(default)]
    pub stop_reason: StopReason,
    /// The failure text, when the response failed.
    #[serde(default)]
    pub error_message: Option<String>,
    /// When the message was created.
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

impl Default for AssistantMessage {
    fn default() -> Self {
        Self {
            role: AssistantRole::Assistant,
            content: Vec::new(),
            api: unknown(),
            provider: unknown(),
            model: unknown(),
            response_model: None,
            response_provider: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            timing: None,
            stop_reason: StopReason::default(),
            error_message: None,
            timestamp: current_timestamp_ms(),
        }
    }
}

impl AssistantMessage {
    /// The concatenated text blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The concatenated reasoning blocks.
    pub fn thinking_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Thinking(thinking) => Some(thinking.thinking.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The requested tool calls, in order.
    pub fn tool_calls(&self) -> Vec<&ToolCall> {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect()
    }
}

/// The outcome of one tool call, as stored in the transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolResultMessage {
    /// Always `toolResult`.
    #[serde(default)]
    pub role: ToolResultRole,
    /// The id of the tool call this answers.
    pub tool_call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The result content: text and images.
    #[serde(default)]
    pub content: Vec<ToolResultContent>,
    /// Structured details for frontends, not sent to the model.
    #[serde(default)]
    pub details: Option<JsonValue>,
    /// Tools this result introduced, when it did.
    #[serde(default)]
    pub added_tool_names: Option<Vec<String>>,
    /// Whether the call failed.
    #[serde(default)]
    pub is_error: bool,
    /// When the result was recorded.
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

impl Default for ToolResultMessage {
    fn default() -> Self {
        Self {
            role: ToolResultRole::ToolResult,
            tool_call_id: String::new(),
            tool_name: String::new(),
            content: Vec::new(),
            details: None,
            added_tool_names: None,
            is_error: false,
            timestamp: current_timestamp_ms(),
        }
    }
}

impl ToolResultMessage {
    /// The concatenated text blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ToolResultContent::Text(text) => Some(text.text.as_str()),
                ToolResultContent::Image(_) => None,
            })
            .collect()
    }
}

/// A shell command the user ran directly, kept in the transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BashExecutionMessage {
    /// Always `bashExecution`.
    #[serde(default)]
    pub role: BashExecutionRole,
    /// The command line.
    pub command: String,
    /// The captured output.
    pub output: String,
    /// The exit code, when the command finished.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Whether the command was cancelled.
    #[serde(default)]
    pub cancelled: bool,
    /// Whether the output was truncated.
    #[serde(default)]
    pub truncated: bool,
    /// Where the untruncated output was written, when it was.
    #[serde(default)]
    pub full_output_path: Option<String>,
    /// When the command ran.
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
    /// Whether to keep the message out of the model context.
    #[serde(default)]
    pub exclude_from_context: bool,
}

/// A message injected by a frontend or an extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomMessage {
    /// Always `custom`.
    #[serde(default)]
    pub role: CustomRole,
    /// The extension-defined kind.
    pub custom_type: String,
    /// The message content.
    pub content: UserContent,
    /// Whether a frontend should render it.
    #[serde(default)]
    pub display: bool,
    /// Structured details for frontends.
    #[serde(default)]
    pub details: Option<JsonValue>,
    /// When the message was created.
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

/// A summary of a transcript branch that was left behind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchSummaryMessage {
    /// Always `branchSummary`.
    #[serde(default)]
    pub role: BranchSummaryRole,
    /// The summary text.
    pub summary: String,
    /// The id of the entry the branch started from.
    pub from_id: String,
    /// When the summary was created.
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

/// A summary produced when older context was compacted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactionSummaryMessage {
    /// Always `compactionSummary`.
    #[serde(default)]
    pub role: CompactionSummaryRole,
    /// The summary text.
    pub summary: String,
    /// The token count before compaction.
    pub tokens_before: u64,
    /// When the summary was created.
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

/// Any message that can appear in a transcript.
///
/// The enum is untagged rather than internally tagged on `role`, because each
/// message already carries a typed `role` marker that both rejects a wrong
/// payload and keeps the message self-describing when it is stored alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentMessage {
    /// A user message.
    User(UserMessage),
    /// An assistant message.
    ///
    /// Boxed because it is by far the largest variant, and a transcript keeps
    /// thousands of messages in memory at once.
    Assistant(Box<AssistantMessage>),
    /// A tool result.
    ToolResult(ToolResultMessage),
    /// A shell execution.
    BashExecution(BashExecutionMessage),
    /// A frontend or extension message.
    Custom(CustomMessage),
    /// A branch summary.
    BranchSummary(BranchSummaryMessage),
    /// A compaction summary.
    CompactionSummary(CompactionSummaryMessage),
}

impl From<UserMessage> for AgentMessage {
    fn from(value: UserMessage) -> Self {
        Self::User(value)
    }
}

impl From<AssistantMessage> for AgentMessage {
    fn from(value: AssistantMessage) -> Self {
        Self::Assistant(Box::new(value))
    }
}

impl From<ToolResultMessage> for AgentMessage {
    fn from(value: ToolResultMessage) -> Self {
        Self::ToolResult(value)
    }
}

impl AgentMessage {
    /// The wire `role` of the message.
    pub fn role(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::Assistant(_) => "assistant",
            Self::ToolResult(_) => "toolResult",
            Self::BashExecution(_) => "bashExecution",
            Self::Custom(_) => "custom",
            Self::BranchSummary(_) => "branchSummary",
            Self::CompactionSummary(_) => "compactionSummary",
        }
    }

    /// The user-visible text this message represents.
    pub fn text(&self) -> String {
        match self {
            Self::User(message) => message.text(),
            Self::Assistant(message) => message.text(),
            Self::ToolResult(message) => message.text(),
            Self::BashExecution(message) => message.output.clone(),
            Self::Custom(message) => message.content.text(),
            Self::BranchSummary(message) => message.summary.clone(),
            Self::CompactionSummary(message) => message.summary.clone(),
        }
    }
}
