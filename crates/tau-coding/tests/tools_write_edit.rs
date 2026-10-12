//! Tests for the `write` and `edit` tools and for the diff `edit` reports.
//!
//! Ports `test_write_tool_creates_parent_directories`,
//! `test_edit_tool_applies_multiple_exact_replacements`,
//! `test_edit_tool_rolls_back_when_any_edit_fails` and
//! `test_edit_tool_requires_unique_matches` from the reference's
//! `tests/test_coding_tools.py`.
//!
//! The diff tests pin a contract the reference leaves untested: its own suite
//! never asserts diff text because `difflib` supplied it. `tools::diff`
//! documents which parts of `difflib` this port reproduces.

use serde_json::json;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_coding::tools::diff::{generate_diff_string, generate_unified_patch};
use tau_coding::tools::{EditTool, WriteTool, create_edit_tool, create_write_tool};
use tau_protocol::{AgentToolResult, JsonObject};

fn arguments(value: serde_json::Value) -> JsonObject {
    value
        .as_object()
        .expect("the test builds a JSON object")
        .clone()
}

async fn write_file(
    tool: &WriteTool,
    value: serde_json::Value,
) -> Result<AgentToolResult, ToolError> {
    let updates = ToolUpdates::new();
    tool.execute("test-call", &arguments(value), &updates).await
}

async fn edit_file(
    tool: &EditTool,
    value: serde_json::Value,
) -> Result<AgentToolResult, ToolError> {
    let updates = ToolUpdates::new();
    tool.execute("test-call", &arguments(value), &updates).await
}

fn error_message(result: Result<AgentToolResult, ToolError>) -> String {
    match result {
        Ok(result) => panic!("expected an error, got {:?}", result.content),
        Err(error) => error.message().to_string(),
    }
}

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).expect("the fixture file reads back")
}

// ---------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_tool_creates_parent_directories() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_write_tool(directory.path());

    let result = write_file(
        &tool,
        json!({"path": "nested/file.txt", "content": "hello"}),
    )
    .await
    .expect("the write succeeds");

    let path = directory.path().join("nested/file.txt");
    assert_eq!(read(&path), "hello");
    assert_eq!(
        result.text(),
        format!("Successfully wrote to {}.", path.display())
    );
    let details = result.details.expect("the write reports details");
    assert_eq!(details["path"], json!(path.display().to_string()));
    assert_eq!(details["characters"], json!(5));
}

#[tokio::test]
async fn write_tool_overwrites_an_existing_file() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "old").expect("the fixture file");
    let tool = create_write_tool(directory.path());

    write_file(&tool, json!({"path": "file.txt", "content": "new"}))
        .await
        .expect("the write succeeds");

    assert_eq!(read(&path), "new");
}

#[tokio::test]
async fn write_tool_reports_characters_not_bytes() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_write_tool(directory.path());

    let result = write_file(&tool, json!({"path": "file.txt", "content": "áé"}))
        .await
        .expect("the write succeeds");

    assert_eq!(read(&directory.path().join("file.txt")), "áé");
    assert_eq!(result.details.expect("details")["characters"], json!(2));
}

#[tokio::test]
async fn write_tool_writes_utf8_bytes_verbatim() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    let tool = create_write_tool(directory.path());

    write_file(&tool, json!({"path": "file.txt", "content": "a\nb\n"}))
        .await
        .expect("the write succeeds");

    // The reference writes through Python's text mode, which would translate
    // every `\n` to the platform separator on Windows. This port writes the
    // bytes it was given.
    assert_eq!(std::fs::read(&path).expect("the bytes"), b"a\nb\n");
}

#[tokio::test]
async fn write_tool_requires_a_string_content() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_write_tool(directory.path());

    let result = write_file(&tool, json!({"path": "file.txt", "content": 4})).await;

    assert_eq!(error_message(result), "content must be a string");
}

// ---------------------------------------------------------------------------
// edit: the ported reference tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_tool_applies_multiple_exact_replacements() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\nbeta\ngamma\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [
                {"oldText": "alpha", "newText": "one"},
                {"oldText": "gamma", "newText": "three"},
            ],
        }),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(read(&path), "one\nbeta\nthree\n");
    assert_eq!(
        result.text(),
        format!("Successfully replaced 2 block(s) in {}.", path.display())
    );
    let details = result.details.expect("the edit reports details");
    assert_eq!(details["path"], json!(path.display().to_string()));
    assert_eq!(details["edits"], json!(2));
    assert_eq!(
        details["diff"],
        json!("- alpha\n+ one\n  beta\n- gamma\n+ three")
    );
    assert_eq!(details["first_changed_line"], json!(1));
    assert_eq!(
        details["patch"],
        json!(format!(
            "--- {path}\n+++ {path}\n@@ -1,3 +1,3 @@\n-alpha\n+one\n beta\n-gamma\n+three\n",
            path = path.display()
        ))
    );
}

#[tokio::test]
async fn edit_tool_rolls_back_when_any_edit_fails() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    let original = "alpha\nbeta\ngamma\n";
    std::fs::write(&path, original).expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [
                {"oldText": "alpha", "newText": "one"},
                {"oldText": "missing", "newText": "nope"},
            ],
        }),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "Could not find edits[1] in {}. The oldText must match exactly including all \
             whitespace and newlines.",
            path.display()
        )
    );
    assert_eq!(read(&path), original);
    assert_eq!(
        std::fs::read(&path).expect("the bytes"),
        original.as_bytes()
    );
}

#[tokio::test]
async fn edit_tool_requires_unique_matches() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "repeat\nrepeat\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [{"oldText": "repeat", "newText": "once"}],
        }),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "Found 2 occurrences of the text in {}. The text must be unique. Please provide more \
             context to make it unique.",
            path.display()
        )
    );
    assert_eq!(read(&path), "repeat\nrepeat\n");
}

// ---------------------------------------------------------------------------
// edit: line endings and byte-order marks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_tool_keeps_a_lf_file_in_lf() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\nbeta\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "beta", "newText": "gamma"}]}),
    )
    .await
    .expect("the edit succeeds");

    // Python's text mode would rewrite these bytes as CRLF on Windows.
    assert_eq!(std::fs::read(&path).expect("the bytes"), b"alpha\ngamma\n");
}

#[tokio::test]
async fn edit_tool_restores_a_crlf_file() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\r\nbeta\r\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "beta", "newText": "gamma"}]}),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(
        std::fs::read(&path).expect("the bytes"),
        b"alpha\r\ngamma\r\n"
    );
}

#[tokio::test]
async fn edit_tool_prefers_lf_when_it_appears_first() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\nbeta\r\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "beta", "newText": "gamma"}]}),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(std::fs::read(&path).expect("the bytes"), b"alpha\ngamma\n");
}

#[tokio::test]
async fn edit_tool_matches_an_old_text_that_carries_crlf() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\r\nbeta\r\ngamma\r\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [{"oldText": "beta\r\n", "newText": "gamma\r\n"}],
        }),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(
        std::fs::read(&path).expect("the bytes"),
        b"alpha\r\ngamma\r\ngamma\r\n"
    );
}

#[tokio::test]
async fn edit_tool_preserves_a_byte_order_mark() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "\u{feff}alpha\r\nbeta\r\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "beta", "newText": "gamma"}]}),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(
        std::fs::read(&path).expect("the bytes"),
        b"\xef\xbb\xbfalpha\r\ngamma\r\n"
    );
    // The diff is computed from the BOM-free, LF-normalized base.
    let details = result.details.expect("details");
    assert_eq!(details["diff"], json!("  alpha\n- beta\n+ gamma"));
    assert_eq!(details["first_changed_line"], json!(2));
}

// ---------------------------------------------------------------------------
// edit: legacy argument shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_tool_parses_an_edits_json_string() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": "[{\"oldText\": \"alpha\", \"newText\": \"one\"}]",
        }),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(read(&path), "one\n");
    assert_eq!(result.details.expect("details")["edits"], json!(1));
}

#[tokio::test]
async fn edit_tool_accepts_a_top_level_old_and_new_text() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    edit_file(
        &tool,
        json!({"path": "file.txt", "oldText": "alpha", "newText": "one"}),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(read(&path), "one\n");
}

#[tokio::test]
async fn edit_tool_appends_a_legacy_pair_to_the_edits_list() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\nbeta\ngamma\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [{"oldText": "alpha", "newText": "one"}],
            "oldText": "gamma",
            "newText": "three",
        }),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(read(&path), "one\nbeta\nthree\n");
    assert_eq!(result.details.expect("details")["edits"], json!(2));
}

#[tokio::test]
async fn edit_tool_leaves_a_malformed_edits_string_alone() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": "[{\"oldText\": }]"}),
    )
    .await;

    assert_eq!(
        error_message(result),
        "Edit tool input is invalid. edits must contain at least one replacement."
    );
    assert_eq!(read(&path), "alpha\n");
}

#[tokio::test]
async fn edit_tool_leaves_an_edits_string_that_is_not_a_list_alone() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": "{\"oldText\": \"alpha\"}"}),
    )
    .await;

    assert_eq!(
        error_message(result),
        "Edit tool input is invalid. edits must contain at least one replacement."
    );
}

#[tokio::test]
async fn edit_tool_ignores_a_legacy_pair_without_a_string_new_text() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(&tool, json!({"path": "file.txt", "oldText": "alpha"})).await;

    assert_eq!(
        error_message(result),
        "Edit tool input is invalid. edits must contain at least one replacement."
    );
}

// ---------------------------------------------------------------------------
// edit: validation order and error text
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_tool_reports_a_missing_file() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "missing.txt",
            "edits": [{"oldText": "alpha", "newText": "one"}],
        }),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "Could not edit file: {}. File not found.",
            directory.path().join("missing.txt").display()
        )
    );
}

#[tokio::test]
async fn edit_tool_reports_a_directory() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    std::fs::create_dir(directory.path().join("sub")).expect("the fixture directory");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "sub",
            "edits": [{"oldText": "alpha", "newText": "one"}],
        }),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "Could not edit file: {}. Path is a directory.",
            directory.path().join("sub").display()
        )
    );
}

#[tokio::test]
async fn edit_tool_rejects_an_empty_edits_list() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(&tool, json!({"path": "file.txt", "edits": []})).await;

    assert_eq!(
        error_message(result),
        "Edit tool input is invalid. edits must contain at least one replacement."
    );
}

#[tokio::test]
async fn edit_tool_rejects_an_edit_item_that_is_not_an_object() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(&tool, json!({"path": "file.txt", "edits": ["alpha"]})).await;

    assert_eq!(error_message(result), "edits[0] must be an object");
}

#[tokio::test]
async fn edit_tool_rejects_edit_texts_that_are_not_strings() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "alpha", "newText": 1}]}),
    )
    .await;

    assert_eq!(
        error_message(result),
        "edits[0].oldText and edits[0].newText must be strings"
    );
}

#[tokio::test]
async fn edit_tool_uses_the_single_edit_error_text_for_one_edit() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "missing", "newText": "x"}]}),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "Could not find the exact text in {}. The old text must match exactly including all \
             whitespace and newlines.",
            path.display()
        )
    );
}

#[tokio::test]
async fn edit_tool_uses_the_multi_edit_duplicate_error_text() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "repeat\nrepeat\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [
                {"oldText": "repeat", "newText": "once"},
                {"oldText": "repeat", "newText": "twice"},
            ],
        }),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "Found 2 occurrences of edits[0] in {}. Each oldText must be unique. Please provide \
             more context to make it unique.",
            path.display()
        )
    );
}

#[tokio::test]
async fn edit_tool_uses_the_single_edit_empty_old_text_error_text() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "", "newText": "x"}]}),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!("oldText must not be empty in {}.", path.display())
    );
}

#[tokio::test]
async fn edit_tool_checks_every_old_text_before_matching_any() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [
                {"oldText": "", "newText": "x"},
                {"oldText": "missing", "newText": "y"},
            ],
        }),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!("edits[0].oldText must not be empty in {}.", path.display())
    );
}

#[tokio::test]
async fn edit_tool_rejects_overlapping_spans() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "abcdef\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [
                {"oldText": "abc", "newText": "X"},
                {"oldText": "bcd", "newText": "Y"},
            ],
        }),
    )
    .await;

    assert_eq!(error_message(result), "Edits must not overlap");
    assert_eq!(read(&path), "abcdef\n");
}

#[tokio::test]
async fn edit_tool_uses_the_single_edit_no_change_error_text() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "alpha", "newText": "alpha"}]}),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "No changes made to {}. The replacement produced identical content. This might \
             indicate an issue with special characters or the text not existing as expected.",
            path.display()
        )
    );
}

#[tokio::test]
async fn edit_tool_uses_the_multi_edit_no_change_error_text() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "alpha\nbeta\n").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({
            "path": "file.txt",
            "edits": [
                {"oldText": "alpha", "newText": "alpha"},
                {"oldText": "beta", "newText": "beta"},
            ],
        }),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "No changes made to {}. The replacements produced identical content.",
            path.display()
        )
    );
}

#[tokio::test]
async fn edit_tool_counts_non_overlapping_occurrences() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "aaa").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    // `aa` occurs once in `aaa` when the search advances by the whole match,
    // which is how the reference counts.
    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "aa", "newText": "b"}]}),
    )
    .await
    .expect("the edit succeeds");

    assert_eq!(read(&path), "ba");
    assert_eq!(result.details.expect("details")["edits"], json!(1));
}

#[tokio::test]
async fn edit_tool_rejects_a_second_non_overlapping_occurrence() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("file.txt");
    std::fs::write(&path, "aaaa").expect("the fixture file");
    let tool = create_edit_tool(directory.path());

    let result = edit_file(
        &tool,
        json!({"path": "file.txt", "edits": [{"oldText": "aa", "newText": "b"}]}),
    )
    .await;

    assert_eq!(
        error_message(result),
        format!(
            "Found 2 occurrences of the text in {}. The text must be unique. Please provide more \
             context to make it unique.",
            path.display()
        )
    );
    assert_eq!(read(&path), "aaaa");
}

// ---------------------------------------------------------------------------
// diff: generate_diff_string
// ---------------------------------------------------------------------------

#[test]
fn diff_string_renders_an_insertion() {
    let (diff, first_changed_line) = generate_diff_string("a\nb\n", "a\nx\nb\n");

    assert_eq!(diff, "  a\n+ x\n  b");
    assert_eq!(first_changed_line, Some(2));
}

#[test]
fn diff_string_renders_a_deletion() {
    let (diff, first_changed_line) = generate_diff_string("a\nb\nc\n", "a\nc\n");

    assert_eq!(diff, "  a\n- b\n  c");
    assert_eq!(first_changed_line, Some(2));
}

#[test]
fn diff_string_renders_a_replacement() {
    let (diff, first_changed_line) = generate_diff_string("a\nbeta\nc\n", "a\ngamma\nc\n");

    assert_eq!(diff, "  a\n- beta\n+ gamma\n  c");
    assert_eq!(first_changed_line, Some(2));
}

#[test]
fn diff_string_renders_a_multiline_change() {
    let (diff, first_changed_line) = generate_diff_string(
        "alpha\nbeta\ngamma\ndelta\nepsilon\n",
        "alpha\nBETA\ngamma\ndelta\nEPSILON\n",
    );

    assert_eq!(
        diff,
        "  alpha\n- beta\n+ BETA\n  gamma\n  delta\n- epsilon\n+ EPSILON"
    );
    assert_eq!(first_changed_line, Some(2));
}

#[test]
fn diff_string_reports_no_change_for_identical_content() {
    let (diff, first_changed_line) = generate_diff_string("a\nb\n", "a\nb\n");

    assert_eq!(diff, "  a\n  b");
    assert_eq!(first_changed_line, None);
}

#[test]
fn diff_string_numbers_the_first_changed_line_for_an_insertion_at_the_head() {
    let (_, first_changed_line) = generate_diff_string("b\n", "a\nb\n");

    assert_eq!(first_changed_line, Some(1));
}

#[test]
fn diff_string_numbers_the_first_changed_line_for_a_replacement_at_the_head() {
    let (_, first_changed_line) = generate_diff_string("a\nb\n", "X\nb\n");

    assert_eq!(first_changed_line, Some(1));
}

#[test]
fn diff_string_numbers_the_first_changed_line_for_an_insertion_at_the_tail() {
    let (_, first_changed_line) = generate_diff_string("a\n", "a\nb\n");

    assert_eq!(first_changed_line, Some(2));
}

#[test]
fn diff_string_numbers_the_first_changed_line_when_the_old_content_is_empty() {
    let (diff, first_changed_line) = generate_diff_string("", "a\n");

    assert_eq!(diff, "+ a");
    assert_eq!(first_changed_line, Some(1));
}

#[test]
fn diff_string_numbers_the_first_changed_line_when_the_new_content_is_empty() {
    let (diff, first_changed_line) = generate_diff_string("a\n", "");

    assert_eq!(diff, "- a");
    assert_eq!(first_changed_line, Some(1));
}

#[test]
fn diff_string_splits_lines_the_way_python_does() {
    // `str::lines` would leave CR, VT and FF as ordinary characters here.
    let (diff, first_changed_line) =
        generate_diff_string("a\rb\u{b}c\u{c}d\n", "a\rb\u{b}c\u{c}d\ne\n");

    assert_eq!(diff, "  a\n  b\n  c\n  d\n+ e");
    assert_eq!(first_changed_line, Some(5));
}

// ---------------------------------------------------------------------------
// diff: generate_unified_patch
// ---------------------------------------------------------------------------

#[test]
fn unified_patch_renders_an_insertion() {
    assert_eq!(
        generate_unified_patch("file.txt", "a\nb\n", "a\nx\nb\n"),
        "--- file.txt\n+++ file.txt\n@@ -1,2 +1,3 @@\n a\n+x\n b\n"
    );
}

#[test]
fn unified_patch_renders_a_deletion() {
    assert_eq!(
        generate_unified_patch("file.txt", "a\nb\nc\n", "a\nc\n"),
        "--- file.txt\n+++ file.txt\n@@ -1,3 +1,2 @@\n a\n-b\n c\n"
    );
}

#[test]
fn unified_patch_renders_a_replacement() {
    assert_eq!(
        generate_unified_patch("file.txt", "a\nbeta\nc\n", "a\ngamma\nc\n"),
        "--- file.txt\n+++ file.txt\n@@ -1,3 +1,3 @@\n a\n-beta\n+gamma\n c\n"
    );
}

#[test]
fn unified_patch_renders_a_multiline_change() {
    assert_eq!(
        generate_unified_patch(
            "file.txt",
            "alpha\nbeta\ngamma\ndelta\nepsilon\n",
            "alpha\nBETA\ngamma\ndelta\nEPSILON\n",
        ),
        "--- file.txt\n+++ file.txt\n@@ -1,5 +1,5 @@\n alpha\n-beta\n+BETA\n gamma\n delta\n\
         -epsilon\n+EPSILON\n"
    );
}

#[test]
fn unified_patch_is_empty_when_nothing_changed() {
    assert_eq!(generate_unified_patch("file.txt", "a\nb\n", "a\nb\n"), "");
}

#[test]
fn unified_patch_formats_an_empty_side_with_a_zero_length_range() {
    assert_eq!(
        generate_unified_patch("file.txt", "", "a\n"),
        "--- file.txt\n+++ file.txt\n@@ -0,0 +1 @@\n+a\n"
    );
    assert_eq!(
        generate_unified_patch("file.txt", "a\n", ""),
        "--- file.txt\n+++ file.txt\n@@ -1 +0,0 @@\n-a\n"
    );
}

#[test]
fn unified_patch_keeps_the_line_terminators_of_its_input() {
    assert_eq!(
        generate_unified_patch("file.txt", "a\r\nb\r\n", "a\r\nc\r\n"),
        "--- file.txt\n+++ file.txt\n@@ -1,2 +1,2 @@\n a\r\n-b\r\n+c\r\n"
    );
}

#[test]
fn unified_patch_reproduces_the_reference_line_handling() {
    // `difflib` writes each line with the terminator it arrived with, and tau
    // feeds it `splitlines(keepends=True)` lines, so a final line with no
    // terminator is emitted as-is and the next patch line is glued onto it.
    // Verified against CPython 3.12.10, which yields exactly " a\n-b+c".
    // Reproduced, not corrected: the reference's patch text is the contract.
    assert_eq!(
        generate_unified_patch("file.txt", "a\nb", "a\nc"),
        "--- file.txt\n+++ file.txt\n@@ -1,2 +1,2 @@\n a\n-b+c"
    );
}

// ---------------------------------------------------------------------------
// declarations
// ---------------------------------------------------------------------------

#[test]
fn write_spec_names_the_tool_and_requires_both_arguments() {
    let spec = tau_coding::tools::write::create_write_tool_spec();

    assert_eq!(spec.name, "write");
    assert_eq!(spec.label, "write");
    assert_eq!(spec.parameters["required"], json!(["path", "content"]));
    assert_eq!(
        spec.prompt_snippet.as_deref(),
        Some("Create or overwrite files")
    );
}

// ---------------------------------------------------------------------------
// diff: triangulation beyond the reference's own cases
// ---------------------------------------------------------------------------

fn count_prefixed(text: &str, prefix: &str) -> usize {
    text.lines().filter(|line| line.starts_with(prefix)).count()
}

/// Two changes further apart than twice the context split into two hunks, and
/// each hunk trims its context to three lines.
///
/// The expected text is `difflib.ndiff` and `difflib.unified_diff` output for
/// this input, read from Python 3.12.
#[test]
fn unified_patch_splits_two_distant_changes_into_two_hunks() {
    let lines = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
        "juliet", "kilo", "lima", "mike", "november", "oscar", "papa", "quebec", "romeo", "sierra",
        "tango",
    ];
    let old = format!("{}\n", lines.join("\n"));
    let mut changed = lines;
    changed[2] = "zulu";
    changed[12] = "yankee";
    let new = format!("{}\n", changed.join("\n"));

    let expected_patch = "\
--- file.txt
+++ file.txt
@@ -1,6 +1,6 @@
 alpha
 bravo
-charlie
+zulu
 delta
 echo
 foxtrot
@@ -10,7 +10,7 @@
 juliet
 kilo
 lima
-mike
+yankee
 november
 oscar
 papa
";

    assert_eq!(
        generate_unified_patch("file.txt", &old, &new),
        expected_patch
    );

    let (diff, first_changed_line) = generate_diff_string(&old, &new);
    assert_eq!(
        diff,
        "  alpha\n  bravo\n- charlie\n+ zulu\n  delta\n  echo\n  foxtrot\n  golf\n  hotel\n  india\n  \
         juliet\n  kilo\n  lima\n- mike\n+ yankee\n  november\n  oscar\n  papa\n  quebec\n  \
         romeo\n  sierra\n  tango"
    );
    assert_eq!(first_changed_line, Some(3));
}

/// From 200 lines on, `SequenceMatcher` excludes a line it sees more than
/// `n / 100 + 1` times, so a file of one repeated line can no longer match
/// itself and the *opcodes* collapse into one replacement.
///
/// `difflib.ndiff` hides that collapse, because `_fancy_replace` re-pairs
/// identical lines as context. CPython 3.12.10 therefore prints 251 lines with
/// one deletion and one insertion for the 250-line `above` input, while this
/// port prints the collapsed opcodes: 375 lines with 125 deletions and 125
/// insertions. The counts pinned here are this port's own, not `difflib`'s.
/// `first_changed_line` is the one field that agrees with the reference in both
/// of these cases.
#[test]
fn diff_string_shows_the_autojunk_collapse_that_difflib_hides() {
    let below_old = "common\n".repeat(199);
    let below_new = format!(
        "{}{}{}",
        "common\n".repeat(99),
        "other\n",
        "common\n".repeat(99)
    );
    let (below, below_first) = generate_diff_string(&below_old, &below_new);

    assert_eq!(below.lines().count(), 200);
    assert_eq!(count_prefixed(&below, "- "), 1);
    assert_eq!(count_prefixed(&below, "+ "), 1);
    assert_eq!(count_prefixed(&below, "+ other"), 1);
    assert_eq!(below_first, Some(100));

    let above_old = "common\n".repeat(250);
    let above_new = format!(
        "{}{}{}",
        "common\n".repeat(125),
        "other\n",
        "common\n".repeat(124)
    );
    let (above, above_first) = generate_diff_string(&above_old, &above_new);

    assert!(above.starts_with("  common\n"));
    assert_eq!(above.lines().count(), 375);
    assert_eq!(count_prefixed(&above, "- "), 125);
    assert_eq!(count_prefixed(&above, "+ "), 125);
    assert_eq!(count_prefixed(&above, "+ other"), 1);
    assert_eq!(above_first, Some(126));
}

/// A replacement whose lines resemble each other is a plain dump here.
///
/// `difflib.ndiff` would instead emit `?         ^` hint lines and pair the
/// lines up character by character; this port emits the `"  "`, `"- "` and
/// `"+ "` lines only.
#[test]
fn diff_string_documents_the_omitted_intra_line_hints() {
    let (diff, first_changed_line) = generate_diff_string("value = 1\n", "value = 2\n");

    assert_eq!(diff, "- value = 1\n+ value = 2");
    assert_eq!(first_changed_line, Some(1));
}
