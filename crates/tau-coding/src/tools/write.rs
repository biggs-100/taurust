//! The `write` tool.
//!
//! Ported from tau's `create_write_tool_definition`. The executor writes the
//! supplied `content` to `path`, resolving a relative path against the tool's
//! working directory, creates any missing parent directory and overwrites an
//! existing file. The whole mutation happens under the per-path lock
//! [`file_lock`] hands out, so two tools cannot interleave on one resolved file.

use std::path::{Path, PathBuf};

use serde_json::json;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_protocol::{AgentToolResult, JsonObject, ToolSpec};

use crate::tools::support::{file_lock, path_arg, str_arg};

/// The `write` tool.
pub struct WriteTool {
    spec: ToolSpec,
    cwd: PathBuf,
}

impl WriteTool {
    /// The directory relative paths resolve against.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

#[tau_agent::async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        let path = path_arg(arguments, "path", &self.cwd)?;
        let content = str_arg(arguments, "content")?;

        {
            let _guard = file_lock(&path).await;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io_error)?;
            }
            // The reference writes through Python's text mode, which translates
            // every `\n` to the platform separator on Windows. The bytes given
            // are written verbatim here instead.
            std::fs::write(&path, &content).map_err(io_error)?;
        }

        let details = json!({
            "path": path.display().to_string(),
            "characters": content.chars().count(),
        });
        Ok(AgentToolResult {
            details: Some(details),
            ..AgentToolResult::from(format!("Successfully wrote to {}.", path.display()))
        })
    }
}

/// The declaration the model sees for `write`.
pub fn create_write_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "write".to_string(),
        label: "write".to_string(),
        description: "Write content to a file. Creates the file if it doesn't exist, overwrites \
                      if it does. Automatically creates parent directories."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the file to write"},
                "content": {"type": "string", "description": "Content to write to the file"},
            },
            "required": ["path", "content"],
        }),
        prompt_snippet: Some("Create or overwrite files".to_string()),
        prompt_guidelines: vec!["Use write only for new files or complete rewrites.".to_string()],
    }
}

/// Create a `write` tool.
pub fn create_write_tool(cwd: impl AsRef<Path>) -> WriteTool {
    WriteTool {
        spec: create_write_tool_spec(),
        cwd: cwd.as_ref().to_path_buf(),
    }
}

/// Report a filesystem failure the way an exception's message would surface.
fn io_error(error: std::io::Error) -> ToolError {
    ToolError::new(error.to_string())
}
