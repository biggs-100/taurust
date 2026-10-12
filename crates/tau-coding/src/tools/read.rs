//! The `read` tool.
//!
//! Ported from tau's `create_read_tool_definition` in `tau_coding/tools.py`.
//! The tool reads a file resolved relative to `cwd`, returns supported images
//! as provider-neutral image blocks, and truncates text output to the shared
//! line or byte ceiling.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_protocol::{AgentToolResult, ImageContent, JsonObject, JsonValue, TextContent, ToolSpec};

use crate::tools::image_processing::{
    DEFAULT_MAX_SOURCE_IMAGE_BYTES, ImageLimits, detect_image_family_mime_type,
    detect_supported_image_mime_type, process_image, unsupported_image_reason,
};
use crate::tools::support::{
    DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_MAX_OUTPUT_LINES, format_size, optional_int_arg, path_arg,
    str_arg, truncate_head,
};

/// How much of a file is sniffed for an image signature before reading it.
pub const IMAGE_SNIFF_BYTES: usize = 64 * 1024;

/// The active-model image capability shared with the built-in tools.
///
/// The state is a tri-state: `None` means the capability is not yet known, and
/// the reference processes images in that case.
#[derive(Debug, Clone)]
pub struct ImageSupportState {
    supported: Arc<Mutex<Option<bool>>>,
}

impl ImageSupportState {
    /// An unknown capability.
    pub fn new() -> Self {
        Self::with_supported(None)
    }

    /// A capability already known.
    pub fn with_supported(supported: Option<bool>) -> Self {
        Self {
            supported: Arc::new(Mutex::new(supported)),
        }
    }

    /// Read the current capability.
    pub fn supported(&self) -> Option<bool> {
        *self
            .supported
            .lock()
            .expect("the image support lock is never poisoned by this crate")
    }

    /// Replace the current capability.
    pub fn set_supported(&self, supported: Option<bool>) {
        *self
            .supported
            .lock()
            .expect("the image support lock is never poisoned by this crate") = supported;
    }
}

impl Default for ImageSupportState {
    fn default() -> Self {
        Self::new()
    }
}

/// The pluggable filesystem operations the read tool uses.
///
/// The reference exposes this as a dataclass of callables so its tests can
/// substitute the filesystem. A trait is the same seam without the closures.
pub trait ReadOperations: Send + Sync {
    /// Reject paths that cannot be read.
    fn validate_path(&self, path: &Path) -> Result<(), ToolError>;

    /// Read the whole file.
    fn read_bytes(&self, path: &Path) -> std::io::Result<Vec<u8>>;

    /// The file size, when the implementation can report it cheaply.
    fn size_bytes(&self, _path: &Path) -> Option<std::io::Result<u64>> {
        None
    }

    /// The first `limit` bytes, when the implementation can read them cheaply.
    fn read_prefix(&self, _path: &Path, _limit: usize) -> Option<std::io::Result<Vec<u8>>> {
        None
    }
}

/// The read operations that touch the local filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalReadOperations;

impl ReadOperations for LocalReadOperations {
    fn validate_path(&self, path: &Path) -> Result<(), ToolError> {
        if !path.exists() {
            return Err(ToolError::new(format!(
                "File not found: {}",
                path.display()
            )));
        }
        if path.is_dir() {
            return Err(ToolError::new(format!(
                "Path is a directory: {}",
                path.display()
            )));
        }
        Ok(())
    }

    fn read_bytes(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn size_bytes(&self, path: &Path) -> Option<std::io::Result<u64>> {
        Some(std::fs::metadata(path).map(|metadata| metadata.len()))
    }

    fn read_prefix(&self, path: &Path, limit: usize) -> Option<std::io::Result<Vec<u8>>> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) => return Some(Err(error)),
        };
        let mut buffer = Vec::new();
        Some(
            file.take(limit as u64)
                .read_to_end(&mut buffer)
                .map(|_| buffer),
        )
    }
}

/// The `read` tool.
pub struct ReadTool {
    spec: ToolSpec,
    cwd: PathBuf,
    operations: Arc<dyn ReadOperations>,
    image_support: Option<ImageSupportState>,
}

impl ReadTool {
    /// The directory relative paths resolve against.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

#[tau_agent::async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        let raw_path = str_arg(arguments, "path")?;
        let path = path_arg(arguments, "path", &self.cwd)?;
        let offset = optional_int_arg(arguments, "offset")?;
        let limit = optional_int_arg(arguments, "limit")?;

        // The reference validates the numeric controls before it touches the
        // filesystem, so an invalid offset reports ahead of a missing file.
        if let Some(offset) = offset {
            if offset < 0 {
                return Err(ToolError::new("offset must be at least 0"));
            }
        }
        if let Some(limit) = limit {
            if limit < 1 {
                return Err(ToolError::new("limit must be at least 1"));
            }
        }

        self.operations.validate_path(&path)?;

        // Reject an oversized image from its sniffed prefix before paying for
        // the full read. Only possible when the operations expose a cheap size
        // and prefix read.
        if let Some(size_result) = self.operations.size_bytes(&path) {
            let source_size = size_result.map_err(io_error)?;
            if source_size > DEFAULT_MAX_SOURCE_IMAGE_BYTES as u64 {
                if let Some(prefix_result) = self.operations.read_prefix(&path, IMAGE_SNIFF_BYTES) {
                    let prefix = prefix_result.map_err(io_error)?;
                    if let Some(image_family) = detect_image_family_mime_type(&prefix) {
                        let reason = unsupported_image_reason(&prefix).unwrap_or_else(|| {
                            format!(
                                "source is {}, exceeding the {} processing limit",
                                format_size(source_size as usize),
                                format_size(DEFAULT_MAX_SOURCE_IMAGE_BYTES)
                            )
                        });
                        return Ok(omitted_image_result(
                            &path,
                            image_family,
                            source_size as usize,
                            &reason,
                        ));
                    }
                }
            }
        }

        let data = self.operations.read_bytes(&path).map_err(io_error)?;

        if let Some(reason) = unsupported_image_reason(&data) {
            let image_family = detect_image_family_mime_type(&data)
                .expect("a recognized unsupported variant always has a known family");
            return Ok(omitted_image_result(
                &path,
                image_family,
                data.len(),
                &reason,
            ));
        }

        if let Some(source_mime_type) = detect_supported_image_mime_type(&data) {
            let source_bytes = data.len();
            if self
                .image_support
                .as_ref()
                .and_then(ImageSupportState::supported)
                == Some(false)
            {
                return Ok(omitted_image_result(
                    &path,
                    source_mime_type,
                    source_bytes,
                    "current model does not support image input. Image contents are \
                     unavailable; do not infer or describe them. Ask the user to switch to a \
                     vision-capable model",
                ));
            }

            // `process_image` is synchronous and CPU-bound; the reference runs
            // it off the event loop, so this runs it on the blocking pool.
            let processed = tokio::task::spawn_blocking(move || {
                process_image(&data, source_mime_type, ImageLimits::default())
            })
            .await
            .map_err(|error| ToolError::new(format!("image processing task failed: {error}")))?;

            let mut details = serde_json::Map::new();
            details.insert("path".to_string(), json!(path.display().to_string()));
            details.insert("source_mime_type".to_string(), json!(source_mime_type));
            details.insert("bytes".to_string(), json!(source_bytes));

            let processed = match processed {
                Ok(processed) => processed,
                Err(failure) => {
                    return Ok(AgentToolResult {
                        content: vec![
                            TextContent {
                                text: format!(
                                    "Read image file [{source_mime_type}]\n[Image omitted: {}.]",
                                    failure.message
                                ),
                                text_signature: None,
                            }
                            .into(),
                        ],
                        details: Some(JsonValue::Object(details)),
                        ..Default::default()
                    });
                }
            };

            details.insert("mime_type".to_string(), json!(processed.mime_type));
            details.insert("processed_bytes".to_string(), json!(processed.data.len()));
            details.insert("width".to_string(), json!(processed.width));
            details.insert("height".to_string(), json!(processed.height));
            let note_lines: String = processed
                .notes
                .iter()
                .map(|note| format!("\n[{note}]"))
                .collect();
            return Ok(AgentToolResult {
                content: vec![
                    TextContent {
                        text: format!("Read image file [{}]{note_lines}", processed.mime_type),
                        text_signature: None,
                    }
                    .into(),
                    ImageContent {
                        data: base64_encode(&processed.data),
                        mime_type: processed.mime_type.clone(),
                    }
                    .into(),
                ],
                details: Some(JsonValue::Object(details)),
                ..Default::default()
            });
        }

        let text = String::from_utf8(data)
            .map_err(|error| ToolError::new(format!("could not decode file as UTF-8: {error}")))?;
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let all_lines: Vec<&str> = text.split('\n').collect();
        let start_line = match offset {
            None | Some(0) => 0usize,
            Some(value) => (value - 1) as usize,
        };
        if start_line >= all_lines.len() {
            return Err(ToolError::new(format!(
                "Offset {} is beyond end of file ({} lines total)",
                offset.expect("a start line past the end implies an explicit offset"),
                all_lines.len()
            )));
        }

        let mut user_limited_lines: Option<usize> = None;
        let selected: String;
        if let Some(limit) = limit {
            let end_line = start_line
                .saturating_add(limit as usize)
                .min(all_lines.len());
            selected = all_lines[start_line..end_line].join("\n");
            user_limited_lines = Some(end_line - start_line);
        } else {
            selected = all_lines[start_line..].join("\n");
        }

        let truncation = truncate_head(
            &selected,
            DEFAULT_MAX_OUTPUT_LINES,
            DEFAULT_MAX_OUTPUT_BYTES,
        );
        let start_display = start_line + 1;
        let details = json!({
            "path": path.display().to_string(),
            "truncation": truncation.to_json(),
        });

        let output = if truncation.first_line_exceeds_limit {
            let first_line_size = format_size(all_lines[start_line].len());
            format!(
                "[Line {start_display} is {first_line_size}, exceeds {} limit. Use bash: sed -n \
                 '{start_display}p' {raw_path} | head -c {DEFAULT_MAX_OUTPUT_BYTES}]",
                format_size(DEFAULT_MAX_OUTPUT_BYTES)
            )
        } else if truncation.truncated {
            let end_display = start_display + truncation.output_lines - 1;
            let next_offset = end_display + 1;
            let total = all_lines.len();
            let mut output = truncation.content.clone();
            if truncation.truncated_by == Some("lines") {
                output.push_str(&format!(
                    "\n\n[Showing lines {start_display}-{end_display} of {total}. Use \
                     offset={next_offset} to continue.]"
                ));
            } else {
                output.push_str(&format!(
                    "\n\n[Showing lines {start_display}-{end_display} of {total} ({} limit). \
                     Use offset={next_offset} to continue.]",
                    format_size(DEFAULT_MAX_OUTPUT_BYTES)
                ));
            }
            output
        } else if let Some(user_limited_lines) = user_limited_lines {
            if start_line + user_limited_lines < all_lines.len() {
                let remaining = all_lines.len() - (start_line + user_limited_lines);
                let next_offset = start_line + user_limited_lines + 1;
                format!(
                    "{}\n\n[{remaining} more lines in file. Use offset={next_offset} to \
                     continue.]",
                    truncation.content
                )
            } else {
                truncation.content.clone()
            }
        } else {
            truncation.content.clone()
        };

        Ok(AgentToolResult {
            content: vec![
                TextContent {
                    text: output,
                    text_signature: None,
                }
                .into(),
            ],
            details: Some(details),
            ..Default::default()
        })
    }
}

/// The declaration the model sees for `read`.
pub fn create_read_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "read".to_string(),
        label: "read".to_string(),
        description: format!(
            "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, \
             bmp). Images are sent to vision-capable models as attachments. For text files, \
             output is truncated to {DEFAULT_MAX_OUTPUT_LINES} lines or {}KB (whichever is hit \
             first). Use offset/limit for large files. When you need the full file, continue \
             with offset until complete.",
            DEFAULT_MAX_OUTPUT_BYTES / 1024
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the file to read"},
                "offset": {"type": "integer", "description": "Line number to start reading from"},
                "limit": {"type": "integer", "description": "Maximum number of lines to read"},
            },
            "required": ["path"],
        }),
        prompt_snippet: Some("Read file contents".to_string()),
        prompt_guidelines: vec!["Use read to examine files instead of cat or sed.".to_string()],
    }
}

/// Create a `read` tool.
pub fn create_read_tool(
    cwd: impl AsRef<Path>,
    operations: Option<Arc<dyn ReadOperations>>,
    image_support: Option<ImageSupportState>,
) -> ReadTool {
    ReadTool {
        spec: create_read_tool_spec(),
        cwd: cwd.as_ref().to_path_buf(),
        operations: operations.unwrap_or_else(|| Arc::new(LocalReadOperations)),
        image_support,
    }
}

/// The omitted-image result shared by the four not-attached outcomes.
fn omitted_image_result(
    path: &Path,
    source_mime_type: &str,
    source_bytes: usize,
    reason: &str,
) -> AgentToolResult {
    AgentToolResult {
        content: vec![
            TextContent {
                text: format!("Read image file [{source_mime_type}]\n[Image omitted: {reason}.]"),
                text_signature: None,
            }
            .into(),
        ],
        details: Some(json!({
            "path": path.display().to_string(),
            "source_mime_type": source_mime_type,
            "bytes": source_bytes,
        })),
        ..Default::default()
    }
}

/// Surface an I/O failure the way the reference's escaping exception does.
fn io_error(error: std::io::Error) -> ToolError {
    ToolError::new(error.to_string())
}

/// Standard base64 encoding of the bytes handed to a model as an attachment.
///
/// The workspace has no base64 dependency and the manifests are frozen, so the
/// small, fixed-alphabet algorithm lives here.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(chunk.get(1).copied().unwrap_or(0));
        let third = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (first << 16) | (second << 8) | third;
        output.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    /// Pin the hand-written encoder to the reference's encoder.
    ///
    /// The reference attaches images with `base64.b64encode`. The workspace has
    /// no base64 dependency, so these vectors were produced by CPython 3.12.10
    /// and the comparison is against the real thing, never against this
    /// encoder's own inverse.
    #[test]
    fn base64_encoding_matches_python_base64_b64encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"\x89PNG\r\n\x1a\n"), "iVBORw0KGgo=");
        assert_eq!(
            base64_encode(&(0u8..=255).collect::<Vec<u8>>()),
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4\
             OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3Bx\
             cnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2en6ChoqOkpaanqKmq\
             q6ytrq+wsbKztLW2t7i5uru8vb6/wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t/g4eLj\
             5OXm5+jp6uvs7e7v8PHy8/T19vf4+fr7/P3+/w=="
        );
    }
}
