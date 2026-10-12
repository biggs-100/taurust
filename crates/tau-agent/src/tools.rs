//! The tool execution protocol.
//!
//! A tool is three things: a declaration the model sees, a few properties that
//! decide how the loop schedules it, and an executor. The declaration lives in
//! [`ToolSpec`], which is the part a provider serializes; everything else is
//! this trait.
//!
//! The reference declares `render_call` and `render_result` on the tool as well.
//! Those are a frontend concern — they are consumed by the extensions runtime to
//! draw a tool invocation — so they arrive with the tool cards rather than here,
//! where nothing would call them.

use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;

use tau_protocol::{AgentToolResult, JsonObject, ToolExecutionMode, ToolSpec};

/// Why a tool could not produce a result.
///
/// The loop turns this into an error result for the transcript, exactly as the
/// reference turns any raised exception into one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    /// An error described by `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message shown to the model and the user.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

/// The partial results a tool reports while it is still running.
///
/// The reference threads a callback that appends to a list the loop owns. A
/// shared sink is the same thing without the borrow gymnastics, and it means the
/// tool hands over owned values instead of the deep copies the reference has to
/// make.
#[derive(Debug, Default)]
pub struct ToolUpdates {
    partials: Mutex<Vec<AgentToolResult>>,
}

impl ToolUpdates {
    /// An empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a partial result.
    pub fn emit(&self, partial: AgentToolResult) {
        self.partials
            .lock()
            .expect("the updates lock is never poisoned by this crate")
            .push(partial);
    }

    /// Take every partial result recorded so far, leaving the collector empty.
    pub fn take(&self) -> Vec<AgentToolResult> {
        std::mem::take(
            &mut *self
                .partials
                .lock()
                .expect("the updates lock is never poisoned by this crate"),
        )
    }

    /// Whether anything has been recorded since the last [`ToolUpdates::take`].
    pub fn is_empty(&self) -> bool {
        self.partials
            .lock()
            .expect("the updates lock is never poisoned by this crate")
            .is_empty()
    }
}

/// A tool the agent loop can schedule and run.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The declaration the model sees.
    fn spec(&self) -> &ToolSpec;

    /// How the loop may schedule this tool against others.
    ///
    /// Declared here to match the reference, which carries the property on every
    /// tool. The reference's own loop runs calls in order regardless, so nothing
    /// consults this yet.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Parallel
    }

    /// Normalize or reject the arguments before the tool runs.
    ///
    /// The default accepts whatever the model sent. An override returns
    /// [`ToolError`] to refuse the call, which the loop reports as an error
    /// result rather than running the tool.
    fn prepare_arguments(&self, raw: &JsonObject) -> Result<JsonObject, ToolError> {
        Ok(raw.clone())
    }

    /// Run one validated tool call.
    ///
    /// Progress goes to `updates`; the return value is the final result. An
    /// [`Err`] means the tool failed, not that it is unavailable.
    async fn execute(
        &self,
        call_id: &str,
        arguments: &JsonObject,
        updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError>;
}
