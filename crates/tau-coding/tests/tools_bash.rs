//! Tests for the `bash` tool.
//!
//! Mirrors the reference tests `test_bash_tool_tolerates_missing_required_display_description`,
//! `test_bash_tool_disconnects_stdin_from_commands`,
//! `test_create_coding_tools_applies_shell_command_prefix`,
//! `test_bash_tool_applies_opt_in_shell_command_prefix`,
//! `test_bash_tool_reports_timeout`, `test_bash_tool_timeout_kills_shell_children`
//! and `test_bash_tool_cancellation_kills_shell_children`.
//!
//! `test_bash_tool_schema_requires_display_description` lives in
//! `tests/tools_catalog.rs`; the spec this file drives has to satisfy it.
//!
//! The reference tests use POSIX shell features (`printf`, `cat`, `sleep`,
//! `touch`, `shopt`, `(cmd) & wait`). Everything that needs them is `#[cfg(unix)]`
//! and the runtime behaviours have `#[cfg(windows)]` twins built from `cmd`.
//! The `shell_command_prefix` tests stay unix-only: the prefix is joined to the
//! command with a newline, which only a POSIX shell reads as a second statement.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
#[cfg(unix)]
use tau_coding::tools::create_coding_tools;
use tau_coding::tools::{BashTool, create_bash_tool};
use tau_protocol::{AgentToolResult, CancellationToken, JsonObject, ManualCancellation};

fn object(value: serde_json::Value) -> JsonObject {
    value
        .as_object()
        .expect("the test builds a JSON object")
        .clone()
}

async fn execute(tool: &BashTool, value: serde_json::Value) -> Result<AgentToolResult, ToolError> {
    let updates = ToolUpdates::new();
    tool.execute("test-call", &object(value), &updates).await
}

async fn run(tool: &BashTool, command: &str) -> AgentToolResult {
    execute(tool, json!({ "command": command }))
        .await
        .expect("the command runs")
}

#[tokio::test]
async fn bash_rejects_a_non_positive_timeout() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let error = execute(&tool, json!({ "command": "echo hi", "timeout": 0 }))
        .await
        .expect_err("a zero timeout is rejected");

    assert_eq!(error.message(), "timeout must be greater than 0");
}

#[tokio::test]
async fn bash_rejects_a_pre_cancelled_command() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let token = Arc::new(ManualCancellation::default());
    token.cancel();
    let tool = create_bash_tool(
        directory.path(),
        None,
        Some(Arc::clone(&token) as Arc<dyn CancellationToken>),
    );

    let error = execute(&tool, json!({ "command": "echo hi" }))
        .await
        .expect_err("a cancelled token rejects the call before spawning");

    assert_eq!(error.message(), "Command cancelled");
}

#[cfg(unix)]
#[tokio::test]
async fn bash_tolerates_a_missing_required_display_description() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let result = run(&tool, "printf hello").await;

    assert_eq!(result.text(), "hello");
    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["exit_code"], json!(0));
    assert_eq!(details["timed_out"], json!(false));
    assert_eq!(details["cancelled"], json!(false));
    assert_eq!(details["shell_command_prefix_applied"], json!(false));
}

#[cfg(unix)]
#[tokio::test]
async fn bash_disconnects_stdin_from_commands() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    // `cat` reads stdin until EOF. With the null device it returns at once; with
    // an inherited stdin it would block until the timeout below.
    let result = execute(&tool, json!({ "command": "cat", "timeout": 5.0 }))
        .await
        .expect("the command runs");

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["timed_out"], json!(false), "{}", result.text());
    assert_eq!(result.text(), "(no output)");
}

#[cfg(unix)]
#[tokio::test]
async fn create_coding_tools_applies_the_shell_command_prefix() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tools = create_coding_tools(
        directory.path(),
        Some("shopt -s expand_aliases\nalias greet='printf coding-tool-alias'"),
        None,
    );
    let bash_tool = tools
        .iter()
        .find(|tool| tool.spec().name == "bash")
        .expect("the catalog contains the bash tool");
    let updates = ToolUpdates::new();

    let result = bash_tool
        .execute(
            "test-call",
            &object(json!({ "command": "greet" })),
            &updates,
        )
        .await
        .expect("the command runs");

    assert_eq!(result.text(), "coding-tool-alias");
    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["shell_command_prefix_applied"], json!(true));
}

#[cfg(unix)]
#[tokio::test]
async fn bash_applies_the_opt_in_shell_command_prefix() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let rc_path = directory.path().join(".zshrc");
    let marker = directory.path().join("sourced");
    std::fs::write(
        &rc_path,
        format!(
            "alias greet='printf alias-output'\ntouch '{}'\n",
            marker.display()
        ),
    )
    .expect("the rc file is written");
    let prefix = format!(
        "shopt -s expand_aliases\neval \"$(grep '^alias ' '{}')\"",
        rc_path.display()
    );
    let tool = create_bash_tool(directory.path(), Some(&prefix), None);

    let result = run(&tool, "greet").await;

    assert_eq!(result.text(), "alias-output");
    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["shell_command_prefix_applied"], json!(true));
    assert!(!marker.exists(), "the prefix must not source the rc file");
}

// The reference is `prefix = shell_command_prefix.strip() if shell_command_prefix
// else None`, so a whitespace-only prefix survives `strip()` as `""`, which is
// not `None`. The two decisions that empty prefix feeds are separate: it *is*
// applied (the command becomes `"\n{command}"` and
// `shell_command_prefix_applied` stays `true`), while `executable="bash" if
// prefix else None` keeps `/bin/sh`, because `""` is falsy.
#[cfg(unix)]
#[tokio::test]
async fn bash_keeps_a_whitespace_only_shell_command_prefix() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    // `$0` is the `argv[0]` the tool spawned, so the probe reports which
    // executable was selected rather than a bash-specific variable. That keeps
    // the assertion true on any host, including one where `/bin/sh` is bash.
    // The names are the ones the tool passes; if the shell is ever switched to
    // the reference's absolute `/bin/sh`, this expectation moves with it.
    let probe = "printf '%s' \"$0\"";

    let control = create_bash_tool(directory.path(), Some("true"), None);
    assert_eq!(run(&control, probe).await.text(), "bash");

    let tool = create_bash_tool(directory.path(), Some("  "), None);
    let result = run(&tool, probe).await;

    assert_eq!(result.text(), "sh");
    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["shell_command_prefix_applied"], json!(true));
}

// The same prefix on the Windows shell. `cmd /C` runs only the first line of a
// multi-line argument, so the empty prefix line means the command is never
// reached: that, together with the flag, is what shows the prefix survived.
#[cfg(windows)]
#[tokio::test]
async fn bash_keeps_a_whitespace_only_shell_command_prefix() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), Some("  "), None);

    let result = run(&tool, "echo hello").await;

    assert_eq!(result.text(), "(no output)");
    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["shell_command_prefix_applied"], json!(true));
}

#[cfg(unix)]
#[tokio::test]
async fn bash_reports_a_timeout() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let result = execute(&tool, json!({ "command": "sleep 1", "timeout": 0.01 }))
        .await
        .expect("the command runs");

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["timed_out"], json!(true));
    assert_eq!(details["cancelled"], json!(false));
    assert!(
        result
            .text()
            .contains("Command timed out after 0.01 seconds"),
        "{}",
        result.text()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn bash_timeout_kills_the_shell_children() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let started = Instant::now();
    let result = execute(
        &tool,
        json!({ "command": "(sleep 0.25; touch marker) & wait", "timeout": 0.01 }),
    )
    .await
    .expect("the command runs");
    let duration = started.elapsed();
    tokio::time::sleep(Duration::from_millis(350)).await;

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["timed_out"], json!(true));
    assert!(duration < Duration::from_millis(500), "{duration:?}");
    assert!(
        !directory.path().join("marker").exists(),
        "the killed process group must not keep running in the background"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn bash_cancellation_kills_the_shell_children() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let token = Arc::new(ManualCancellation::default());
    let tool = Arc::new(create_bash_tool(
        directory.path(),
        None,
        Some(Arc::clone(&token) as Arc<dyn CancellationToken>),
    ));
    let runner = Arc::clone(&tool);

    let task = tokio::spawn(async move {
        let updates = ToolUpdates::new();
        runner
            .execute(
                "test-call",
                &object(json!({ "command": "sleep 1 & wait" })),
                &updates,
            )
            .await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    token.cancel();
    let started = Instant::now();
    let result = task
        .await
        .expect("the tool task completes")
        .expect("the command runs");
    let duration = started.elapsed();

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["cancelled"], json!(true));
    assert!(
        result.text().contains("Command cancelled"),
        "{}",
        result.text()
    );
    assert!(duration < Duration::from_millis(500), "{duration:?}");
}

// A child that closes its own output and then outlives the deadline is still
// timed out, and promptly: the reference's `Process.communicate()` waits for the
// process to exit as well as for pipe EOF, so its deadline stays armed. The
// shell redirects itself, which closes every copy of the merged output socket
// before `sleep` starts, and the leftover `sleep` keeps running until the kill.
//
// There is no Windows twin for this: the tool always runs `cmd /C`, and `cmd`
// holds its own stdout and stderr handles for its whole life (and passes them to
// its children), so on Windows a command can never close the pipes while the
// shell is still running. Both this port and the reference therefore wait for
// the orphan that holds them, and the measured reference on this host takes 148
// seconds for `ping -n 30 127.0.0.1 >NUL 2>&1` with a 0.25 second timeout.
#[cfg(unix)]
#[tokio::test]
async fn bash_times_out_a_child_that_closed_its_own_output() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let started = Instant::now();
    let result = execute(
        &tool,
        json!({ "command": "exec >/dev/null 2>&1; sleep 30", "timeout": 0.25 }),
    )
    .await
    .expect("the command runs");
    let duration = started.elapsed();

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["timed_out"], json!(true), "{}", result.text());
    assert_eq!(details["cancelled"], json!(false));
    assert!(duration < Duration::from_secs(5), "{duration:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn bash_reports_the_full_output_path_when_truncated() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let result = run(&tool, "seq 1 5000").await;

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["truncation"]["truncated"], json!(true));
    assert_eq!(details["truncation"]["truncated_by"], json!("lines"));
    assert!(
        result.text().contains("[Showing lines 3001-5000 of 5000."),
        "{}",
        result.text()
    );
    let path = details["full_output_path"]
        .as_str()
        .expect("the full output is saved");
    let written = std::fs::read_to_string(path).expect("the saved output is readable");
    assert!(written.starts_with("1\n"), "{written}");
    assert!(written.ends_with("5000\n"), "{written}");
    let _ = std::fs::remove_file(path);
}

#[cfg(windows)]
#[tokio::test]
async fn bash_tolerates_a_missing_required_display_description() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let result = run(&tool, "echo hello").await;

    assert_eq!(result.text().trim_end(), "hello");
    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["exit_code"], json!(0));
    assert_eq!(details["timed_out"], json!(false));
    assert_eq!(details["cancelled"], json!(false));
    assert_eq!(details["shell_command_prefix_applied"], json!(false));
}

#[cfg(windows)]
#[tokio::test]
async fn bash_disconnects_stdin_from_commands() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    // `sort` reads stdin until EOF. With the null device it returns at once.
    let result = execute(&tool, json!({ "command": "sort", "timeout": 5.0 }))
        .await
        .expect("the command runs");

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["timed_out"], json!(false), "{}", result.text());
    assert_eq!(result.text(), "(no output)");
}

// `shell_command_prefix` is joined to the command with a newline, which is a
// POSIX-shell statement separator. `cmd /C` runs only the first line of a
// multi-line argument, so the prefix feature is exercised on unix only; wiring
// it for `cmd` would be a deviation from the reference's `_prefixed_shell_command`.

// `cmd.exe` has no backslash escape for a double quote, so the command has to
// reach it as a raw argument. Rust's argv quoting would send `echo \"a\"`, which
// `cmd` echoes verbatim instead of running the quotes as written.
#[cfg(windows)]
#[tokio::test]
async fn bash_round_trips_double_quotes_on_windows() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let echoed = run(&tool, r#"echo "a""#).await;
    assert_eq!(echoed.text().trim_end(), "\"a\"");
}

// An external command proves the quotes reach an argument parser intact: a shell
// builtin would echo the mangled text back, but `where.exe` searches for the
// pattern it is handed, so a `\"`-escaped `cmd` finds nothing.
#[cfg(windows)]
#[tokio::test]
async fn bash_passes_a_quoted_argument_to_an_external_command_on_windows() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let found = run(&tool, r#"where "cmd""#).await;
    let details = found.details.as_ref().expect("the tool reports details");
    assert_eq!(details["exit_code"], json!(0), "{}", found.text());

    let listed = run(&tool, r#"dir "C:\Windows""#).await;
    let details = listed.details.as_ref().expect("the tool reports details");
    assert_eq!(details["exit_code"], json!(0), "{}", listed.text());
}

// A `for /l` loop runs inside `cmd` itself, with no grandchild to hold the
// output pipes after the shell is killed. That makes it the right probe for the
// Windows fallback, which terminates the direct child only.
#[cfg(windows)]
const WINDOWS_LONG_COMMAND: &str = "for /l %i in (1,1,2147483647) do @rem";

#[cfg(windows)]
#[tokio::test]
async fn bash_reports_a_timeout() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let result = execute(
        &tool,
        json!({ "command": WINDOWS_LONG_COMMAND, "timeout": 0.01 }),
    )
    .await
    .expect("the command runs");

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["timed_out"], json!(true));
    assert_eq!(details["cancelled"], json!(false));
    assert!(
        result
            .text()
            .contains("Command timed out after 0.01 seconds"),
        "{}",
        result.text()
    );
}

// Windows only kills the direct child; killing grandchildren is a POSIX
// process-group behaviour and is exercised by the `#[cfg(unix)]` twin above.
// A fast return here is the evidence that the shell itself was terminated: with
// no grandchild, the output pipes close only when `cmd` dies.
#[cfg(windows)]
#[tokio::test]
async fn bash_timeout_stops_the_shell() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let started = Instant::now();
    let result = execute(
        &tool,
        json!({ "command": WINDOWS_LONG_COMMAND, "timeout": 0.05 }),
    )
    .await
    .expect("the command runs");
    let duration = started.elapsed();

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["timed_out"], json!(true));
    assert!(duration < Duration::from_millis(500), "{duration:?}");
}

#[cfg(windows)]
#[tokio::test]
async fn bash_cancellation_stops_the_shell() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let token = Arc::new(ManualCancellation::default());
    let tool = Arc::new(create_bash_tool(
        directory.path(),
        None,
        Some(Arc::clone(&token) as Arc<dyn CancellationToken>),
    ));
    let runner = Arc::clone(&tool);

    let task = tokio::spawn(async move {
        let updates = ToolUpdates::new();
        runner
            .execute(
                "test-call",
                &object(json!({ "command": WINDOWS_LONG_COMMAND })),
                &updates,
            )
            .await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    token.cancel();
    let started = Instant::now();
    let result = task
        .await
        .expect("the tool task completes")
        .expect("the command runs");
    let duration = started.elapsed();

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["cancelled"], json!(true));
    assert!(
        result.text().contains("Command cancelled"),
        "{}",
        result.text()
    );
    assert!(duration < Duration::from_millis(500), "{duration:?}");
}

#[cfg(windows)]
#[tokio::test]
async fn bash_reports_the_full_output_path_when_truncated() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let tool = create_bash_tool(directory.path(), None, None);

    let result = run(&tool, "for /l %i in (1,1,5000) do @echo %i").await;

    let details = result.details.as_ref().expect("the tool reports details");
    assert_eq!(details["truncation"]["truncated"], json!(true));
    assert_eq!(details["truncation"]["truncated_by"], json!("lines"));
    assert!(
        result.text().contains("[Showing lines 3001-5000 of 5000."),
        "{}",
        result.text()
    );
    let path = details["full_output_path"]
        .as_str()
        .expect("the full output is saved");
    let written = std::fs::read_to_string(path).expect("the saved output is readable");
    assert!(written.starts_with("1\r\n"), "{written}");
    assert!(written.ends_with("5000\r\n"), "{written}");
    let _ = std::fs::remove_file(path);
}
