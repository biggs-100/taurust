//! The tool execution protocol.
//!
//! The reference declares more on a tool than the loop consumes: `execution_mode`
//! and `prepare_arguments` are declared and propagated, and the renderers are
//! consumed by the extensions runtime for display. The renderers are a frontend
//! concern and arrive with the tool cards; everything else is here.

use serde_json::json;
use tau_agent::async_trait;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_protocol::{AgentToolResult, JsonObject, ToolExecutionMode, ToolSpec};

pub(crate) fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        label: name.to_string(),
        description: format!("{name} tool"),
        parameters: json!({ "type": "object" }),
        prompt_snippet: None,
        prompt_guidelines: Vec::new(),
    }
}

pub(crate) fn args(value: serde_json::Value) -> JsonObject {
    value.as_object().expect("a JSON object").clone()
}

/// A tool that reports two partial results and echoes its `text` argument.
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
        updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        updates.emit(AgentToolResult::from("first"));
        updates.emit(AgentToolResult::from("second"));

        let text = arguments
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        Ok(AgentToolResult::from(text))
    }
}

/// A tool that reports no progress, refuses to run without `text`, and must run alone.
struct StrictTool {
    spec: ToolSpec,
}

#[async_trait]
impl Tool for StrictTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }

    fn prepare_arguments(&self, raw: &JsonObject) -> Result<JsonObject, ToolError> {
        match raw.get("text").and_then(|value| value.as_str()) {
            Some(text) => Ok(args(json!({ "text": text }))),
            None => Err(ToolError::new("text is required")),
        }
    }

    async fn execute(
        &self,
        _call_id: &str,
        _arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        Ok(AgentToolResult::from("ok"))
    }
}

/// A tool that always fails.
struct FailingTool {
    spec: ToolSpec,
}

#[async_trait]
impl Tool for FailingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        _arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        Err(ToolError::new("boom"))
    }
}

fn echo() -> EchoTool {
    EchoTool { spec: spec("echo") }
}

#[tokio::test]
async fn a_tool_returns_the_result_of_its_execution() {
    let updates = ToolUpdates::new();

    let result = echo()
        .execute("call-1", &args(json!({ "text": "hi" })), &updates)
        .await
        .expect("the echo tool succeeds");

    assert_eq!(result.text(), "hi");
}

#[tokio::test]
async fn partial_updates_are_collected_in_order() {
    let updates = ToolUpdates::new();

    let _ = echo()
        .execute("call-1", &args(json!({ "text": "hi" })), &updates)
        .await;

    let partials: Vec<String> = updates
        .take()
        .into_iter()
        .map(|partial| partial.text())
        .collect();
    assert_eq!(partials, vec!["first", "second"]);
}

#[tokio::test]
async fn taking_the_updates_leaves_nothing_behind() {
    let updates = ToolUpdates::new();

    let _ = echo()
        .execute("call-1", &args(json!({ "text": "hi" })), &updates)
        .await;

    assert_eq!(updates.take().len(), 2);
    assert!(updates.take().is_empty());
}

#[tokio::test]
async fn a_tool_that_reports_no_progress_yields_no_updates() {
    let updates = ToolUpdates::new();
    let tool = StrictTool {
        spec: spec("strict"),
    };

    let _ = tool
        .execute("call-1", &args(json!({ "text": "hi" })), &updates)
        .await;

    assert!(updates.take().is_empty());
}

#[tokio::test]
async fn a_failing_tool_reports_its_reason_rather_than_a_result() {
    let updates = ToolUpdates::new();
    let tool = FailingTool {
        spec: spec("failing"),
    };

    let error = tool
        .execute("call-1", &args(json!({})), &updates)
        .await
        .expect_err("the failing tool must fail");

    assert_eq!(error.message(), "boom");
}

/// A tool that does not override the optional parts of the protocol.
struct BareTool {
    spec: ToolSpec,
}

#[async_trait]
impl Tool for BareTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        _arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        Ok(AgentToolResult::from("ok"))
    }
}

#[test]
fn the_default_execution_mode_is_parallel() {
    let tool = BareTool { spec: spec("bare") };

    assert_eq!(tool.execution_mode(), ToolExecutionMode::Parallel);
}

#[test]
fn the_default_argument_preparation_leaves_arguments_unchanged() {
    let tool = BareTool { spec: spec("bare") };
    let arguments = args(json!({ "text": "hi", "extra": 1 }));

    assert_eq!(tool.prepare_arguments(&arguments).unwrap(), arguments);
}

#[test]
fn an_override_reports_sequential_execution() {
    let tool = StrictTool {
        spec: spec("strict"),
    };

    assert_eq!(tool.execution_mode(), ToolExecutionMode::Sequential);
}

#[test]
fn an_argument_preparation_override_normalizes_the_arguments() {
    let tool = StrictTool {
        spec: spec("strict"),
    };

    let prepared = tool
        .prepare_arguments(&args(json!({ "text": "hi", "extra": 1 })))
        .expect("text is present");

    assert_eq!(prepared, args(json!({ "text": "hi" })));
    assert_eq!(
        tool.prepare_arguments(&args(json!({ "extra": 1 })))
            .expect_err("text is required")
            .message(),
        "text is required"
    );
}

#[test]
fn a_tool_exposes_its_specification() {
    let tool = echo();

    assert_eq!(tool.spec().name, "echo");
    assert_eq!(tool.spec().description, "echo tool");
}
