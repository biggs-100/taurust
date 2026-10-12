//! Shared output and argument vocabulary for the built-in tools.
//!
//! Ported from the top-level helpers of tau's `tau_coding/tools.py`: the output
//! truncation contract, the human-readable size format, the structured argument
//! readers every tool validates its input with, and the per-path mutation lock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::json;
use tau_agent::tools::ToolError;
use tau_protocol::{JsonObject, JsonValue};

/// Output ceiling in bytes, whichever output limit is hit first.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 50 * 1024;

/// Output ceiling in lines, whichever output limit is hit first.
pub const DEFAULT_MAX_OUTPUT_LINES: usize = 2_000;

/// Metadata describing how a tool output was shortened.
///
/// `content` is the returned slice. The remaining fields record whether
/// truncation happened, which limit was responsible, the total size of the
/// original output, the size of the returned output, and the edge cases the
/// reference distinguishes: a partial last line, and a first line too large to
/// display safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncationResult {
    /// The returned slice.
    pub content: String,
    /// Whether anything was removed.
    pub truncated: bool,
    /// Which limit was hit: `"lines"` or `"bytes"`.
    pub truncated_by: Option<&'static str>,
    /// Lines in the original output.
    pub total_lines: usize,
    /// Bytes in the original output.
    pub total_bytes: usize,
    /// Lines in the returned slice.
    pub output_lines: usize,
    /// Bytes in the returned slice.
    pub output_bytes: usize,
    /// Whether the returned slice starts inside a line.
    pub last_line_partial: bool,
    /// Whether the first line alone exceeds the byte limit.
    pub first_line_exceeds_limit: bool,
    /// The line limit the result was produced under.
    pub max_lines: usize,
    /// The byte limit the result was produced under.
    pub max_bytes: usize,
}

impl TruncationResult {
    /// The JSON shape the tools report in their details.
    ///
    /// The field names are the reference's `asdict` output, so they are
    /// snake_case and a missing `truncated_by` is `null` rather than absent.
    pub fn to_json(&self) -> JsonValue {
        let mut object = serde_json::Map::new();
        object.insert("content".to_string(), json!(self.content));
        object.insert("truncated".to_string(), json!(self.truncated));
        object.insert(
            "truncated_by".to_string(),
            match self.truncated_by {
                Some(limit) => json!(limit),
                None => JsonValue::Null,
            },
        );
        object.insert("total_lines".to_string(), json!(self.total_lines));
        object.insert("total_bytes".to_string(), json!(self.total_bytes));
        object.insert("output_lines".to_string(), json!(self.output_lines));
        object.insert("output_bytes".to_string(), json!(self.output_bytes));
        object.insert(
            "last_line_partial".to_string(),
            json!(self.last_line_partial),
        );
        object.insert(
            "first_line_exceeds_limit".to_string(),
            json!(self.first_line_exceeds_limit),
        );
        object.insert("max_lines".to_string(), json!(self.max_lines));
        object.insert("max_bytes".to_string(), json!(self.max_bytes));
        JsonValue::Object(object)
    }
}

/// Format a byte count the way the tools report sizes to the model.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    if bytes < 1024 * 1024 {
        return format!("{:.1}KB", bytes as f64 / 1024.0);
    }
    format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
}

/// Keep the head of `content` within `max_lines` and `max_bytes`.
pub fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();
    let total_bytes = content.len();
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return build(
            content.to_string(),
            false,
            None,
            Measurements::whole(total_lines, total_bytes),
            false,
            false,
        );
    }

    let first_line_bytes = lines.first().map_or(0, |line| line.len());
    if first_line_bytes > max_bytes {
        return build(
            String::new(),
            true,
            Some("bytes"),
            Measurements::whole(total_lines, total_bytes).with_output(0, 0),
            false,
            true,
        );
    }

    let mut output_lines: Vec<&str> = Vec::new();
    let mut output_bytes = 0usize;
    let mut truncated_by = "lines";
    for (index, line) in lines.iter().take(max_lines).enumerate() {
        let line_bytes = line.len() + usize::from(index > 0);
        if output_bytes + line_bytes > max_bytes {
            truncated_by = "bytes";
            break;
        }
        output_lines.push(line);
        output_bytes += line_bytes;
    }

    let output = output_lines.join("\n");
    let output_bytes = output.len();
    build(
        output,
        true,
        Some(truncated_by),
        Measurements::whole(total_lines, total_bytes).with_output(output_lines.len(), output_bytes),
        false,
        false,
    )
}

/// Keep the tail of `content` within `max_lines` and `max_bytes`.
pub fn truncate_tail(content: &str, max_lines: usize, max_bytes: usize) -> TruncationResult {
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();
    let total_bytes = content.len();
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return build(
            content.to_string(),
            false,
            None,
            Measurements::whole(total_lines, total_bytes),
            false,
            false,
        );
    }

    let mut output_lines: Vec<&str> = Vec::new();
    let mut head: Option<String> = None;
    let mut output_bytes = 0usize;
    let mut truncated_by = "lines";
    let mut last_line_partial = false;
    for line in lines.iter().rev() {
        let line_bytes = line.len() + usize::from(!output_lines.is_empty());
        if output_lines.len() >= max_lines {
            truncated_by = "lines";
            break;
        }
        if output_bytes + line_bytes > max_bytes {
            truncated_by = "bytes";
            if output_lines.is_empty() {
                let clipped = truncate_string_to_bytes_from_end(line, max_bytes);
                head = Some(clipped);
                last_line_partial = true;
            }
            break;
        }
        output_lines.push(line);
        output_bytes += line_bytes;
    }

    let (output, output_lines) = match head {
        Some(clipped) => (clipped, 1),
        None => {
            let joined = output_lines
                .iter()
                .rev()
                .copied()
                .collect::<Vec<&str>>()
                .join("\n");
            (joined, output_lines.len())
        }
    };
    let output_bytes = output.len();
    build(
        output,
        true,
        Some(truncated_by),
        Measurements::whole(total_lines, total_bytes).with_output(output_lines, output_bytes),
        last_line_partial,
        false,
    )
}

/// Read a required string argument.
pub fn str_arg(arguments: &JsonObject, name: &str) -> Result<String, ToolError> {
    match arguments.get(name) {
        Some(JsonValue::String(value)) => Ok(value.clone()),
        _ => Err(ToolError::new(format!("{name} must be a string"))),
    }
}

/// Read a required path argument, resolving it against `cwd`.
///
/// A leading `~` expands to the home directory, as the reference's
/// `Path.expanduser` does. The reference also expands `~user` on POSIX, which
/// needs a passwd lookup this workspace cannot perform without `unsafe`; a
/// `~user` prefix is left alone instead.
pub fn path_arg(arguments: &JsonObject, name: &str, cwd: &Path) -> Result<PathBuf, ToolError> {
    let value = str_arg(arguments, name)?;
    let path = expand_user(&value);
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

/// Read an optional integer argument.
pub fn optional_int_arg(arguments: &JsonObject, name: &str) -> Result<Option<i64>, ToolError> {
    match arguments.get(name) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| ToolError::new(format!("{name} must be an integer"))),
        _ => Err(ToolError::new(format!("{name} must be an integer"))),
    }
}

/// Read an optional number argument.
pub fn optional_float_arg(arguments: &JsonObject, name: &str) -> Result<Option<f64>, ToolError> {
    match arguments.get(name) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(JsonValue::Number(number)) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| ToolError::new(format!("{name} must be a number"))),
        _ => Err(ToolError::new(format!("{name} must be a number"))),
    }
}

/// The per-path mutation lock shared by `write` and `edit`.
///
/// The reference keeps a process-wide map of per-resolved-path locks so two
/// concurrent mutations of the same file cannot interleave. Keys are never
/// pruned, matching the reference.
static FILE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();

/// Acquire the mutation lock for `path`, holding it until the guard drops.
pub async fn file_lock(path: &Path) -> tokio::sync::OwnedMutexGuard<()> {
    let key = lock_key(path);
    let locks = FILE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let lock = {
        let mut locks = locks
            .lock()
            .expect("the file lock map is never poisoned by this crate");
        Arc::clone(locks.entry(key).or_default())
    };
    lock.lock_owned().await
}

/// The key two spellings of one file must agree on.
///
/// The reference keys the map by `Path.resolve()`, so symlinks collapse. A path
/// that does not exist yet cannot be canonicalized whole, so its parent is, and
/// a path with no resolvable parent falls back to its absolute form.
fn lock_key(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if let Some(parent) = path.parent() {
        if let Ok(canonical_parent) = parent.canonicalize() {
            if let Some(name) = path.file_name() {
                return canonical_parent.join(name);
            }
        }
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Expand a leading `~` to the home directory.
fn expand_user(value: &str) -> PathBuf {
    let Some(rest) = value.strip_prefix('~') else {
        return PathBuf::from(value);
    };
    let Some(home) = dirs::home_dir() else {
        return PathBuf::from(value);
    };
    if rest.is_empty() {
        return home;
    }
    match rest.strip_prefix(['/', '\\']) {
        Some(relative) => home.join(relative),
        None => PathBuf::from(value),
    }
}

/// Split content into lines the way the reference counts them.
///
/// A trailing newline is a separator, not an extra empty line, and empty
/// content has no lines at all.
fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Keep the last `max_bytes` bytes of `text`, dropping a split character.
///
/// The reference slices the encoded bytes and decodes ignoring errors, which
/// drops a leading partial character. Slicing at the next character boundary is
/// the same result without an intermediate lossy decode.
fn truncate_string_to_bytes_from_end(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let start = text.len() - max_bytes;
    let mut boundary = start;
    while boundary < text.len() && !text.is_char_boundary(boundary) {
        boundary += 1;
    }
    text[boundary..].to_string()
}

/// The two sizes of one truncation: the original and what was returned.
struct Measurements {
    total_lines: usize,
    total_bytes: usize,
    output_lines: usize,
    output_bytes: usize,
}

impl Measurements {
    /// Both sizes equal, which is what an untruncated output reports.
    fn whole(total_lines: usize, total_bytes: usize) -> Self {
        Self {
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
        }
    }

    /// Replace the returned size.
    fn with_output(mut self, output_lines: usize, output_bytes: usize) -> Self {
        self.output_lines = output_lines;
        self.output_bytes = output_bytes;
        self
    }
}

/// Build a result, recording the module defaults as the limits in force.
///
/// The reference's `_truncation_result` always writes `DEFAULT_MAX_OUTPUT_LINES`
/// and `DEFAULT_MAX_OUTPUT_BYTES`, ignoring whatever limits the call supplied.
/// That is a real quirk of the module, preserved here and pinned by a test.
fn build(
    content: String,
    truncated: bool,
    truncated_by: Option<&'static str>,
    measurements: Measurements,
    last_line_partial: bool,
    first_line_exceeds_limit: bool,
) -> TruncationResult {
    TruncationResult {
        content,
        truncated,
        truncated_by,
        total_lines: measurements.total_lines,
        total_bytes: measurements.total_bytes,
        output_lines: measurements.output_lines,
        output_bytes: measurements.output_bytes,
        last_line_partial,
        first_line_exceeds_limit,
        max_lines: DEFAULT_MAX_OUTPUT_LINES,
        max_bytes: DEFAULT_MAX_OUTPUT_BYTES,
    }
}
