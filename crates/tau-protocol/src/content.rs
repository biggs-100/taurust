//! Pi-compatible content blocks.
//!
//! A message stores an ordered list of blocks. The list is the single
//! representation; a bare string is only an ergonomic constructor.

use serde::{Deserialize, Serialize};

use crate::json::JsonObject;

/// A block of plain text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextContent {
    /// The text itself.
    pub text: String,
    /// A provider signature attached to the text, when one was returned.
    #[serde(default)]
    pub text_signature: Option<String>,
}

/// A block of model reasoning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThinkingContent {
    /// The reasoning text.
    pub thinking: String,
    /// A provider signature attached to the reasoning, when one was returned.
    #[serde(default)]
    pub thinking_signature: Option<String>,
    /// Whether the provider withheld the reasoning and sent only a marker.
    #[serde(default)]
    pub redacted: bool,
}

/// An image block, carried as base64 data plus its media type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageContent {
    /// Base64-encoded image bytes.
    pub data: String,
    /// The image media type, for example `image/png`.
    pub mime_type: String,
}

/// A tool call requested by the assistant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCall {
    /// The provider-assigned call id, echoed back on the tool result.
    pub id: String,
    /// The tool name.
    pub name: String,
    /// The parsed arguments.
    #[serde(default)]
    pub arguments: JsonObject,
    /// A provider signature accompanying the call, when one was returned.
    #[serde(default)]
    pub thought_signature: Option<String>,
}

/// An assistant content block: text, reasoning or a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssistantContent {
    /// A text block.
    #[serde(rename = "text")]
    Text(TextContent),
    /// A reasoning block.
    #[serde(rename = "thinking")]
    Thinking(ThinkingContent),
    /// A tool call block.
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}

impl From<TextContent> for AssistantContent {
    fn from(value: TextContent) -> Self {
        Self::Text(value)
    }
}

impl From<ThinkingContent> for AssistantContent {
    fn from(value: ThinkingContent) -> Self {
        Self::Thinking(value)
    }
}

impl From<ToolCall> for AssistantContent {
    fn from(value: ToolCall) -> Self {
        Self::ToolCall(value)
    }
}

/// A content block allowed in a tool result: text or image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResultContent {
    /// A text block.
    #[serde(rename = "text")]
    Text(TextContent),
    /// An image block.
    #[serde(rename = "image")]
    Image(ImageContent),
}

impl From<TextContent> for ToolResultContent {
    fn from(value: TextContent) -> Self {
        Self::Text(value)
    }
}

impl From<ImageContent> for ToolResultContent {
    fn from(value: ImageContent) -> Self {
        Self::Image(value)
    }
}

/// A content block allowed in user input: text or image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UserContentBlock {
    /// A text block.
    #[serde(rename = "text")]
    Text(TextContent),
    /// An image block.
    #[serde(rename = "image")]
    Image(ImageContent),
}

impl From<TextContent> for UserContentBlock {
    fn from(value: TextContent) -> Self {
        Self::Text(value)
    }
}

impl From<ImageContent> for UserContentBlock {
    fn from(value: ImageContent) -> Self {
        Self::Image(value)
    }
}

/// User input: either a bare string or an ordered list of blocks.
///
/// The untagged shape is deliberate: Pi's wire format sends a plain JSON string
/// for text-only input and an array once an image is attached.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    /// Text-only input, sent as a JSON string.
    Text(String),
    /// Block input, sent as an array.
    Blocks(Vec<UserContentBlock>),
}

impl Default for UserContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<&str> for UserContent {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for UserContent {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl UserContent {
    /// The visible text of the input, ignoring images.
    pub fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    UserContentBlock::Text(text) => Some(text.text.as_str()),
                    UserContentBlock::Image(_) => None,
                })
                .collect(),
        }
    }
}
