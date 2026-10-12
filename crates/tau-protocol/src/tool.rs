//! Provider-neutral tool declarations and tool results.

use serde::{Deserialize, Serialize};

use crate::content::{ImageContent, TextContent, ToolResultContent};
use crate::json::JsonValue;

/// The final or partial result produced by a tool.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct AgentToolResult {
    /// The result content: text and images.
    pub content: Vec<ToolResultContent>,
    /// Structured details for frontends, never sent to the model.
    pub details: Option<JsonValue>,
    /// Tool names this result introduced, when it introduced any.
    pub added_tool_names: Option<Vec<String>>,
    /// Whether the loop should stop after this result.
    pub terminate: Option<bool>,
}

impl AgentToolResult {
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

    /// The images carried by the result.
    pub fn images(&self) -> Vec<&ImageContent> {
        self.content
            .iter()
            .filter_map(|block| match block {
                ToolResultContent::Image(image) => Some(image),
                ToolResultContent::Text(_) => None,
            })
            .collect()
    }
}

impl From<&str> for AgentToolResult {
    fn from(value: &str) -> Self {
        Self {
            content: if value.is_empty() {
                Vec::new()
            } else {
                vec![
                    TextContent {
                        text: value.to_string(),
                        text_signature: None,
                    }
                    .into(),
                ]
            },
            ..Self::default()
        }
    }
}

impl From<String> for AgentToolResult {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

/// The model-facing declaration of a tool.
///
/// This is the part a provider serializes into a request. Executing the tool is
/// the agent layer's business, so no closure lives here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolSpec {
    /// The tool name the model calls.
    pub name: String,
    /// The human-readable label a frontend shows.
    pub label: String,
    /// The description given to the model.
    pub description: String,
    /// The JSON schema of the arguments.
    pub parameters: JsonValue,
    /// A short line included in the system prompt, when the tool wants one.
    #[serde(default)]
    pub prompt_snippet: Option<String>,
    /// Guidelines included in the system prompt for this tool.
    #[serde(default)]
    pub prompt_guidelines: Vec<String>,
}

/// How a frontend schedules concurrent tool calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolExecutionMode {
    /// The tool may run alongside other tools.
    #[default]
    #[serde(rename = "parallel")]
    Parallel,
    /// The tool must run alone, in call order.
    #[serde(rename = "sequential")]
    Sequential,
}
