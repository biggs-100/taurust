//! The `edit` tool.
//!
//! Ported from tau's `create_edit_tool_definition` plus the helpers below it in
//! `tau_coding/tools.py` that only this tool uses: the line-ending detection and
//! normalization, the validated replacement pipeline, the legacy argument
//! normalization and every error text.
//!
//! Every replacement is validated before anything is written, so a failing edit
//! leaves the file byte-identical. The replacements are applied in reverse order
//! so they cannot shift each other's offsets.

use std::path::{Path, PathBuf};

use serde_json::json;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_protocol::{AgentToolResult, JsonObject, JsonValue, ToolSpec};

use crate::tools::diff::{generate_diff_string, generate_unified_patch};
use crate::tools::support::{file_lock, path_arg};

/// The UTF-8 byte-order mark the tool strips before matching and writes back.
const UTF8_BOM: &str = "\u{feff}";

/// The text a payload without a usable `edits` list is rejected with.
const EDITS_INPUT_ERROR: &str =
    "Edit tool input is invalid. edits must contain at least one replacement.";

/// One replacement in the tool's canonical shape.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Edit {
    old_text: String,
    new_text: String,
}

/// The `edit` tool.
pub struct EditTool {
    spec: ToolSpec,
    cwd: PathBuf,
}

impl EditTool {
    /// The directory relative paths resolve against.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

#[tau_agent::async_trait]
impl Tool for EditTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        let prepared = prepare_edit_arguments(arguments);
        let path = path_arg(&prepared, "path", &self.cwd)?;
        let edits = edits_arg(&prepared)?;
        let display = path.display().to_string();

        if !path.exists() {
            return Err(ToolError::new(format!(
                "Could not edit file: {display}. File not found."
            )));
        }
        if path.is_dir() {
            return Err(ToolError::new(format!(
                "Could not edit file: {display}. Path is a directory."
            )));
        }

        let (base_content, new_content) = {
            let _guard = file_lock(&path).await;
            let raw_content = std::fs::read_to_string(&path).map_err(io_error)?;
            let (bom, content) = strip_bom(&raw_content);
            let original_ending = detect_line_ending(content);
            let normalized = normalize_to_lf(content);
            let (base_content, new_content) =
                apply_edits_to_normalized_content(&normalized, &edits, &display)?;
            // The reference writes through Python's text mode, which would
            // translate every `\n` to the platform separator on Windows. The
            // bytes computed here are written verbatim instead.
            let final_content = format!(
                "{bom}{}",
                restore_line_endings(&new_content, original_ending)
            );
            std::fs::write(&path, final_content).map_err(io_error)?;
            (base_content, new_content)
        };

        let (diff, first_changed_line) = generate_diff_string(&base_content, &new_content);
        let patch = generate_unified_patch(&display, &base_content, &new_content);
        let text = format!(
            "Successfully replaced {} block(s) in {display}.",
            edits.len()
        );
        let details = json!({
            "path": display,
            "edits": edits.len(),
            "diff": diff,
            "patch": patch,
            "first_changed_line": first_changed_line,
        });
        Ok(AgentToolResult {
            details: Some(details),
            ..AgentToolResult::from(text)
        })
    }
}

/// The declaration the model sees for `edit`.
pub fn create_edit_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "edit".to_string(),
        label: "edit".to_string(),
        description: "Edit a single file using exact text replacement. Every edits[].oldText must \
                      match a unique, non-overlapping region of the original file. If two changes \
                      affect the same block or nearby lines, merge them into one edit instead of \
                      emitting overlapping edits. Do not include large unchanged regions just to \
                      connect distant changes."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to the file to edit"},
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {"type": "string"},
                            "newText": {"type": "string"},
                        },
                        "required": ["oldText", "newText"],
                        "additionalProperties": false,
                    },
                },
            },
            "required": ["path", "edits"],
            "additionalProperties": false,
        }),
        prompt_snippet: Some(
            "Make precise file edits with exact text replacement, including multiple disjoint \
             edits in one call"
                .to_string(),
        ),
        prompt_guidelines: vec![
            "Use edit for precise changes (edits[].oldText must match exactly)".to_string(),
            "When changing multiple separate locations in one file, use one edit call with \
             multiple entries in edits[] instead of multiple edit calls"
                .to_string(),
            "Each edits[].oldText is matched against the original file, not after earlier edits \
             are applied. Do not emit overlapping or nested edits. Merge nearby changes into one \
             edit."
                .to_string(),
            "Keep edits[].oldText as small as possible while still being unique in the file. Do \
             not pad with large unchanged regions."
                .to_string(),
        ],
    }
}

/// Create an `edit` tool.
pub fn create_edit_tool(cwd: impl AsRef<Path>) -> EditTool {
    EditTool {
        spec: create_edit_tool_spec(),
        cwd: cwd.as_ref().to_path_buf(),
    }
}

/// Normalize the legacy argument shapes the tool still accepts.
///
/// An `edits` value that is a JSON *string* is parsed, but only a parse that
/// yields a list replaces it; anything else, including malformed JSON, is left
/// untouched for the argument reader to reject. A top-level string `oldText` and
/// `newText` pair is appended to the list as one more edit and removed from the
/// payload.
fn prepare_edit_arguments(arguments: &JsonObject) -> JsonObject {
    let mut prepared = arguments.clone();

    let parsed = match prepared.get("edits") {
        Some(JsonValue::String(text)) => serde_json::from_str::<JsonValue>(text).ok(),
        _ => None,
    };
    if let Some(parsed) = parsed {
        if parsed.is_array() {
            prepared.insert("edits".to_string(), parsed);
        }
    }

    let legacy = match (prepared.get("oldText"), prepared.get("newText")) {
        (Some(JsonValue::String(old_text)), Some(JsonValue::String(new_text))) => {
            Some((old_text.clone(), new_text.clone()))
        }
        _ => None,
    };
    if let Some((old_text, new_text)) = legacy {
        let mut edit_list = match prepared.get("edits") {
            Some(JsonValue::Array(items)) => items.clone(),
            _ => Vec::new(),
        };
        edit_list.push(json!({"oldText": old_text, "newText": new_text}));
        prepared.insert("edits".to_string(), JsonValue::Array(edit_list));
        prepared.remove("oldText");
        prepared.remove("newText");
    }
    prepared
}

/// Read the canonical `edits` list, or explain why it cannot be read.
fn edits_arg(arguments: &JsonObject) -> Result<Vec<Edit>, ToolError> {
    let items = match arguments.get("edits") {
        Some(JsonValue::Array(items)) if !items.is_empty() => items,
        _ => return Err(ToolError::new(EDITS_INPUT_ERROR)),
    };

    let mut edits = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let JsonValue::Object(object) = item else {
            return Err(ToolError::new(format!("edits[{index}] must be an object")));
        };
        match (object.get("oldText"), object.get("newText")) {
            (Some(JsonValue::String(old_text)), Some(JsonValue::String(new_text))) => {
                edits.push(Edit {
                    old_text: old_text.clone(),
                    new_text: new_text.clone(),
                });
            }
            _ => {
                return Err(ToolError::new(format!(
                    "edits[{index}].oldText and edits[{index}].newText must be strings"
                )));
            }
        }
    }
    Ok(edits)
}

/// Validate every edit and return the normalized base and the replaced content.
///
/// The reference's order is kept: every `oldText` is checked for emptiness
/// first, then each one must occur exactly once, then the spans must not
/// overlap, then the result must differ from the original.
fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<(String, String), ToolError> {
    let normalized_edits: Vec<Edit> = edits
        .iter()
        .map(|edit| Edit {
            old_text: normalize_to_lf(&edit.old_text),
            new_text: normalize_to_lf(&edit.new_text),
        })
        .collect();
    let total_edits = normalized_edits.len();

    for (index, edit) in normalized_edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(ToolError::new(empty_old_text_error(
                path,
                index,
                total_edits,
            )));
        }
    }

    let mut matches: Vec<(usize, usize, String)> = Vec::new();
    for (index, edit) in normalized_edits.iter().enumerate() {
        let occurrences = count_occurrences(normalized_content, &edit.old_text);
        if occurrences == 0 {
            return Err(ToolError::new(not_found_error(path, index, total_edits)));
        }
        if occurrences > 1 {
            return Err(ToolError::new(duplicate_error(
                path,
                index,
                total_edits,
                occurrences,
            )));
        }
        let start = normalized_content
            .find(&edit.old_text)
            .expect("the occurrence was counted");
        matches.push((start, start + edit.old_text.len(), edit.new_text.clone()));
    }

    validate_non_overlapping(&matches)?;

    let mut ordered = matches;
    ordered.sort_by(|left, right| right.cmp(left));
    let mut new_content = normalized_content.to_string();
    for (start, end, new_text) in ordered {
        new_content = format!(
            "{}{}{}",
            &new_content[..start],
            new_text,
            &new_content[end..]
        );
    }

    if new_content == normalized_content {
        return Err(ToolError::new(no_change_error(path, total_edits)));
    }
    Ok((normalized_content.to_string(), new_content))
}

/// Reject spans that share any offset.
fn validate_non_overlapping(spans: &[(usize, usize, String)]) -> Result<(), ToolError> {
    let mut sorted: Vec<&(usize, usize, String)> = spans.iter().collect();
    sorted.sort();
    let mut previous_end: Option<usize> = None;
    for (start, end, _new_text) in sorted {
        if let Some(previous) = previous_end {
            if *start < previous {
                return Err(ToolError::new("Edits must not overlap"));
            }
        }
        previous_end = Some(*end);
    }
    Ok(())
}

/// Count non-overlapping occurrences, advancing by the whole match.
///
/// The caller guarantees `text` is not empty; an empty needle would never
/// advance.
fn count_occurrences(content: &str, text: &str) -> usize {
    debug_assert!(
        !text.is_empty(),
        "the empty needle is rejected before matching"
    );
    if text.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut start = 0;
    loop {
        match content[start..].find(text) {
            Some(index) => {
                count += 1;
                start += index + text.len();
            }
            None => return count,
        }
    }
}

/// The dominant line ending of `content`.
///
/// The reference returns CRLF only when a CRLF appears before the first LF, so a
/// file that mixes both takes its ending from whichever comes first.
fn detect_line_ending(content: &str) -> &'static str {
    match (content.find('\n'), content.find("\r\n")) {
        (Some(lf_index), Some(crlf_index)) if crlf_index < lf_index => "\r\n",
        _ => "\n",
    }
}

/// Normalize CRLF and lone CR to LF.
fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Put `ending` back into LF-normalized text.
fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// Split off a leading byte-order mark, reporting it for the write back.
fn strip_bom(content: &str) -> (&'static str, &str) {
    match content.strip_prefix(UTF8_BOM) {
        Some(rest) => (UTF8_BOM, rest),
        None => ("", content),
    }
}

/// The text for an `oldText` that never occurs.
fn not_found_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!(
            "Could not find the exact text in {path}. The old text must match exactly including \
             all whitespace and newlines."
        )
    } else {
        format!(
            "Could not find edits[{edit_index}] in {path}. The oldText must match exactly \
             including all whitespace and newlines."
        )
    }
}

/// The text for an `oldText` that occurs more than once.
fn duplicate_error(
    path: &str,
    edit_index: usize,
    total_edits: usize,
    occurrences: usize,
) -> String {
    if total_edits == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. \
             Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{edit_index}] in {path}. Each oldText must \
             be unique. Please provide more context to make it unique."
        )
    }
}

/// The text for an empty `oldText`.
fn empty_old_text_error(path: &str, edit_index: usize, total_edits: usize) -> String {
    if total_edits == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{edit_index}].oldText must not be empty in {path}.")
    }
}

/// The text for a replacement that produced identical content.
fn no_change_error(path: &str, total_edits: usize) -> String {
    if total_edits == 1 {
        format!(
            "No changes made to {path}. The replacement produced identical content. This might \
             indicate an issue with special characters or the text not existing as expected."
        )
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

/// Report a filesystem failure the way an exception's message would surface.
fn io_error(error: std::io::Error) -> ToolError {
    ToolError::new(error.to_string())
}
