//! Tests for the shared output and argument vocabulary of the built-in tools.
//!
//! `tau_coding/tools.py` has no direct tests for these helpers; the behaviour
//! pinned here is read from the reference implementation itself.

use std::path::Path;
use std::time::Duration;

use tau_coding::tools::support::{
    DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_MAX_OUTPUT_LINES, file_lock, format_size, optional_float_arg,
    optional_int_arg, path_arg, str_arg, truncate_head, truncate_tail,
};
use tau_protocol::JsonObject;

fn object(value: serde_json::Value) -> JsonObject {
    value
        .as_object()
        .expect("the test builds a JSON object")
        .clone()
}

#[test]
fn format_size_uses_the_reference_units() {
    assert_eq!(format_size(0), "0B");
    assert_eq!(format_size(1023), "1023B");
    assert_eq!(format_size(1024), "1.0KB");
    assert_eq!(format_size(1536), "1.5KB");
    assert_eq!(format_size(1024 * 1024 - 1), "1024.0KB");
    assert_eq!(format_size(1024 * 1024), "1.0MB");
    assert_eq!(format_size(1024 * 1024 * 1024), "1024.0MB");
}

#[test]
fn truncate_head_leaves_a_short_output_alone() {
    let result = truncate_head(
        "alpha\nbeta",
        DEFAULT_MAX_OUTPUT_LINES,
        DEFAULT_MAX_OUTPUT_BYTES,
    );

    assert_eq!(result.content, "alpha\nbeta");
    assert!(!result.truncated);
    assert_eq!(result.truncated_by, None);
    assert_eq!(result.total_lines, 2);
    assert_eq!(result.total_bytes, 10);
    assert_eq!(result.output_lines, 2);
    assert_eq!(result.output_bytes, 10);
    assert!(!result.last_line_partial);
    assert!(!result.first_line_exceeds_limit);
}

#[test]
fn truncate_head_counts_a_trailing_newline_as_a_separator() {
    let result = truncate_head(
        "alpha\nbeta\n",
        DEFAULT_MAX_OUTPUT_LINES,
        DEFAULT_MAX_OUTPUT_BYTES,
    );

    assert_eq!(result.total_lines, 2);
    assert_eq!(result.total_bytes, 11);
    assert_eq!(result.content, "alpha\nbeta\n");
}

#[test]
fn truncate_head_stops_at_the_line_limit() {
    let result = truncate_head("one\ntwo\nthree\nfour", 2, DEFAULT_MAX_OUTPUT_BYTES);

    assert_eq!(result.content, "one\ntwo");
    assert!(result.truncated);
    assert_eq!(result.truncated_by, Some("lines"));
    assert_eq!(result.total_lines, 4);
    assert_eq!(result.output_lines, 2);
}

#[test]
fn truncate_head_stops_when_the_next_line_would_exceed_the_byte_limit() {
    let result = truncate_head("one\ntwo\nthree", DEFAULT_MAX_OUTPUT_LINES, 7);

    assert_eq!(result.content, "one\ntwo");
    assert_eq!(result.truncated_by, Some("bytes"));
    assert_eq!(result.output_lines, 2);
    assert_eq!(result.output_bytes, 7);
    assert_eq!(result.total_lines, 3);
}

#[test]
fn truncate_head_reports_a_first_line_over_the_byte_limit() {
    let result = truncate_head("abcdefghij", DEFAULT_MAX_OUTPUT_LINES, 4);

    assert_eq!(result.content, "");
    assert!(result.truncated);
    assert_eq!(result.truncated_by, Some("bytes"));
    assert!(result.first_line_exceeds_limit);
    assert_eq!(result.output_lines, 0);
    assert_eq!(result.output_bytes, 0);
}

#[test]
fn truncation_reports_the_default_limits_even_when_called_with_others() {
    // The reference's `_truncation_result` always records the module defaults,
    // not the limits the call supplied. Preserved deliberately.
    let result = truncate_head("one\ntwo", 1, 4);

    assert_eq!(result.max_lines, DEFAULT_MAX_OUTPUT_LINES);
    assert_eq!(result.max_bytes, DEFAULT_MAX_OUTPUT_BYTES);
}

#[test]
fn truncate_tail_keeps_the_last_lines() {
    let result = truncate_tail("one\ntwo\nthree\nfour", 2, DEFAULT_MAX_OUTPUT_BYTES);

    assert_eq!(result.content, "three\nfour");
    assert!(result.truncated);
    assert_eq!(result.truncated_by, Some("lines"));
    assert_eq!(result.total_lines, 4);
    assert_eq!(result.output_lines, 2);
    assert!(!result.last_line_partial);
}

#[test]
fn truncate_tail_clips_a_single_line_from_the_end() {
    let result = truncate_tail("0123456789", DEFAULT_MAX_OUTPUT_LINES, 4);

    assert_eq!(result.content, "6789");
    assert_eq!(result.truncated_by, Some("bytes"));
    assert_eq!(result.output_bytes, 4);
    assert!(result.last_line_partial);
}

#[test]
fn truncation_json_uses_the_reference_field_names() {
    let truncated = truncate_head("one\ntwo", 1, DEFAULT_MAX_OUTPUT_BYTES);
    let json = truncated.to_json();

    assert_eq!(json["content"], serde_json::json!("one"));
    assert_eq!(json["truncated"], serde_json::json!(true));
    assert_eq!(json["truncated_by"], serde_json::json!("lines"));
    assert_eq!(json["total_lines"], serde_json::json!(2));
    assert_eq!(json["output_lines"], serde_json::json!(1));
    assert_eq!(json["last_line_partial"], serde_json::json!(false));
    assert_eq!(json["first_line_exceeds_limit"], serde_json::json!(false));

    let whole = truncate_head("one", DEFAULT_MAX_OUTPUT_LINES, DEFAULT_MAX_OUTPUT_BYTES);
    assert_eq!(whole.to_json()["truncated_by"], serde_json::Value::Null);
}

#[test]
fn string_reader_names_the_argument_and_its_expected_type() {
    assert_eq!(
        str_arg(&object(serde_json::json!({"path": "a"})), "path").unwrap(),
        "a"
    );

    let error = str_arg(&object(serde_json::json!({"path": 7})), "path").unwrap_err();
    assert_eq!(error.message(), "path must be a string");

    let missing = str_arg(&object(serde_json::json!({})), "path").unwrap_err();
    assert_eq!(missing.message(), "path must be a string");
}

#[test]
fn optional_integer_reader_distinguishes_absent_from_invalid() {
    assert_eq!(
        optional_int_arg(&object(serde_json::json!({"offset": 3})), "offset").unwrap(),
        Some(3)
    );
    assert_eq!(
        optional_int_arg(&object(serde_json::json!({"offset": null})), "offset").unwrap(),
        None
    );
    assert_eq!(
        optional_int_arg(&object(serde_json::json!({})), "offset").unwrap(),
        None
    );

    let error =
        optional_int_arg(&object(serde_json::json!({"offset": "3"})), "offset").unwrap_err();
    assert_eq!(error.message(), "offset must be an integer");
}

#[test]
fn optional_number_reader_accepts_integers_and_numbers() {
    assert_eq!(
        optional_float_arg(&object(serde_json::json!({"timeout": 2})), "timeout").unwrap(),
        Some(2.0)
    );
    assert_eq!(
        optional_float_arg(&object(serde_json::json!({"timeout": 2.5})), "timeout").unwrap(),
        Some(2.5)
    );
    assert_eq!(
        optional_float_arg(&object(serde_json::json!({"timeout": null})), "timeout").unwrap(),
        None
    );

    let error =
        optional_float_arg(&object(serde_json::json!({"timeout": "2"})), "timeout").unwrap_err();
    assert_eq!(error.message(), "timeout must be a number");
}

#[test]
fn path_reader_resolves_relative_paths_against_the_working_directory() {
    let directory = tempfile::tempdir().expect("a temporary directory");

    let resolved = path_arg(
        &object(serde_json::json!({"path": "nested/file.txt"})),
        "path",
        directory.path(),
    )
    .expect("a relative path resolves");

    assert_eq!(resolved, directory.path().join("nested/file.txt"));
}

#[test]
fn path_reader_keeps_absolute_paths() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let absolute = directory.path().join("file.txt");

    let resolved = path_arg(
        &object(serde_json::json!({"path": absolute.to_string_lossy()})),
        "path",
        Path::new("."),
    )
    .expect("an absolute path resolves");

    assert_eq!(resolved, absolute);
}

#[test]
fn path_reader_expands_a_leading_tilde() {
    let Some(home) = dirs::home_dir() else {
        return;
    };

    let resolved = path_arg(
        &object(serde_json::json!({"path": "~/notes.txt"})),
        "path",
        Path::new("."),
    )
    .expect("a home-relative path resolves");

    assert_eq!(resolved, home.join("notes.txt"));
}

#[tokio::test]
async fn file_lock_serializes_two_spellings_of_the_same_path() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let file = directory.path().join("a.txt");
    std::fs::write(&file, "x").expect("the fixture file");

    let held = file_lock(&file).await;
    let spelled_out = directory.path().join(".").join("a.txt");

    let blocked = tokio::time::timeout(Duration::from_millis(100), file_lock(&spelled_out)).await;
    assert!(blocked.is_err(), "one resolved path must serialize");

    drop(held);

    let acquired = tokio::time::timeout(Duration::from_millis(500), file_lock(&spelled_out)).await;
    assert!(acquired.is_ok(), "the guard releases the lock");
}

#[tokio::test]
async fn file_lock_keys_an_unwritten_path_by_its_existing_parent() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let missing = directory.path().join("nested.txt");
    let spelled_out = directory.path().join(".").join("nested.txt");

    let held = file_lock(&missing).await;
    let blocked = tokio::time::timeout(Duration::from_millis(100), file_lock(&spelled_out)).await;
    assert!(blocked.is_err());

    drop(held);
}

#[tokio::test]
async fn file_lock_does_not_serialize_different_paths() {
    let directory = tempfile::tempdir().expect("a temporary directory");

    let held = file_lock(&directory.path().join("a.txt")).await;
    let other = tokio::time::timeout(
        Duration::from_millis(500),
        file_lock(&directory.path().join("b.txt")),
    )
    .await;

    assert!(other.is_ok(), "different paths must not contend");
    drop(held);
}
