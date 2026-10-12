//! The `bash` tool.
//!
//! Ported from tau's `create_bash_tool_definition` and the private helpers only
//! that tool uses: `append_status_block`, `_communicate_with_cancellation`,
//! `_wait_for_cancel`, `_kill_process_tree` and `_write_temp_output`.
//!
//! The reference threads a cancellation token per call. The Rust
//! [`tau_agent::tools::Tool`] trait carries none, so the tool holds the injected
//! token from construction and polls it while the command runs.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_protocol::{AgentToolResult, CancellationToken, JsonObject, ToolSpec};

use super::support::{
    DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_MAX_OUTPUT_LINES, format_size, optional_float_arg, str_arg,
    truncate_tail,
};

/// How often the injected cancellation token is polled while a command runs.
///
/// The reference polls every `0.05` seconds; the timeout is checked on the same
/// tick.
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One chunk of output, or the error that ended the stream.
type ChunkSender = tokio::sync::mpsc::UnboundedSender<std::io::Result<Vec<u8>>>;

/// The `bash` tool.
pub struct BashTool {
    spec: ToolSpec,
    cwd: PathBuf,
    shell_command_prefix: Option<String>,
    cancellation: Option<Arc<dyn CancellationToken>>,
}

impl BashTool {
    /// The directory relative paths and commands resolve against.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

/// How the command finished, in the order the reference decides the status.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The command ran to completion.
    Completed,
    /// The supplied timeout elapsed first.
    TimedOut,
    /// The injected token was cancelled first.
    Cancelled,
}

#[tau_agent::async_trait]
impl Tool for BashTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        let command = str_arg(arguments, "command")?;
        let timeout = optional_float_arg(arguments, "timeout")?;
        if timeout.is_some_and(|timeout| timeout <= 0.0) {
            return Err(ToolError::new("timeout must be greater than 0"));
        }
        if self
            .cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            return Err(ToolError::new("Command cancelled"));
        }

        let prefix = self.shell_command_prefix.as_deref();
        let shell_command = prefixed_shell_command(&command, prefix);
        // The reference takes two decisions out of one prefix, and they are kept
        // separate here: the flag reports whether *a* prefix was configured at
        // all (including a whitespace-only one, which `strip()` reduces to
        // `""`), while the unix executable follows the prefix's truthiness.
        let shell_command_prefix_applied = prefix.is_some();
        let prefix_selects_bash = prefix_is_truthy(prefix);
        let started = Instant::now();

        let (sender, mut receiver) =
            tokio::sync::mpsc::unbounded_channel::<std::io::Result<Vec<u8>>>();

        let mut child: tokio::process::Child;
        #[cfg(unix)]
        {
            let (spawned, reader) =
                spawn_child(&shell_command, prefix_selects_bash, self.cwd.as_path())
                    .map_err(|error| ToolError::new(error.to_string()))?;
            child = spawned;
            spawn_unix_reader(reader, sender.clone());
        }
        #[cfg(windows)]
        {
            child = spawn_child(&shell_command, prefix_selects_bash, self.cwd.as_path())
                .map_err(|error| ToolError::new(error.to_string()))?;
            if let Some(stdout) = child.stdout.take() {
                spawn_async_reader(stdout, sender.clone());
            }
            if let Some(stderr) = child.stderr.take() {
                spawn_async_reader(stderr, sender.clone());
            }
        }
        drop(sender);

        let child_pid = child.id();
        let mut output: Vec<u8> = Vec::new();
        let mut outcome = Outcome::Completed;
        let mut ticker = tokio::time::interval(CANCELLATION_POLL_INTERVAL);
        let watch = timeout.is_some() || self.cancellation.is_some();
        // The reference's `Process.communicate()` waits for the process to exit as
        // well as for pipe EOF, so the deadline and the token stay armed after a
        // command closes its own output streams. The exit is therefore awaited
        // here, independently of the readers' state, and the call returns only
        // once the readers are done *and* the child has exited.
        let mut status: Option<std::process::ExitStatus> = None;
        let mut readers_done = false;
        loop {
            tokio::select! {
                chunk = receiver.recv(), if !readers_done => match chunk {
                    Some(Ok(bytes)) => output.extend_from_slice(&bytes),
                    Some(Err(_)) => continue,
                    None => readers_done = true,
                },
                exit = child.wait(), if status.is_none() => {
                    status = Some(exit.map_err(|error| ToolError::new(error.to_string()))?);
                }
                _ = ticker.tick(), if watch && outcome == Outcome::Completed => {
                    let cancelled = self
                        .cancellation
                        .as_ref()
                        .is_some_and(|token| token.is_cancelled());
                    let expired = timeout
                        .is_some_and(|timeout| started.elapsed().as_secs_f64() >= timeout);
                    if cancelled {
                        outcome = Outcome::Cancelled;
                        kill_process_tree(&mut child, child_pid);
                    } else if expired {
                        outcome = Outcome::TimedOut;
                        kill_process_tree(&mut child, child_pid);
                    }
                }
            }
            if readers_done && status.is_some() {
                break;
            }
        }

        let status = status.expect("the loop exits only once the child has exited");
        let exit_code = process_exit_code(&status);

        let decoded = String::from_utf8_lossy(&output).into_owned();
        let truncation =
            truncate_tail(&decoded, DEFAULT_MAX_OUTPUT_LINES, DEFAULT_MAX_OUTPUT_BYTES);
        let mut output_text = if truncation.content.is_empty() {
            "(no output)".to_string()
        } else {
            truncation.content.clone()
        };
        let mut full_output_path: Option<String> = None;
        if truncation.truncated {
            let path = write_temp_output(&decoded)?;
            full_output_path = Some(path.clone());
            let start_line = truncation.total_lines - truncation.output_lines + 1;
            let end_line = truncation.total_lines;
            if truncation.last_line_partial {
                output_text.push_str(&format!(
                    "\n\n[Showing last {} of line {end_line}. Full output: {path}]",
                    format_size(truncation.output_bytes)
                ));
            } else if truncation.truncated_by == Some("lines") {
                output_text.push_str(&format!(
                    "\n\n[Showing lines {start_line}-{end_line} of {}. Full output: {path}]",
                    truncation.total_lines
                ));
            } else {
                output_text.push_str(&format!(
                    "\n\n[Showing lines {start_line}-{end_line} of {} ({} limit). \
                     Full output: {path}]",
                    truncation.total_lines,
                    format_size(DEFAULT_MAX_OUTPUT_BYTES)
                ));
            }
        }

        let status_text = if outcome == Outcome::TimedOut {
            Some(match timeout {
                Some(timeout) => {
                    format!(
                        "Command timed out after {} seconds",
                        format_general(timeout)
                    )
                }
                None => "Command timed out".to_string(),
            })
        } else if outcome == Outcome::Cancelled {
            Some("Command cancelled".to_string())
        } else {
            match exit_code {
                Some(code) if code != 0 => Some(format!("Command exited with code {code}")),
                _ => None,
            }
        };
        if let Some(status_text) = status_text {
            output_text = append_status_block(&output_text, &status_text);
        }

        let duration_seconds = (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0;
        let details = json!({
            "command": command,
            "exit_code": exit_code,
            "timed_out": outcome == Outcome::TimedOut,
            "cancelled": outcome == Outcome::Cancelled,
            "duration_seconds": duration_seconds,
            "truncation": truncation.to_json(),
            "full_output_path": full_output_path,
            "shell_command_prefix_applied": shell_command_prefix_applied,
        });

        let mut result = AgentToolResult::from(output_text);
        result.details = Some(details);
        Ok(result)
    }
}

/// The declaration the model sees for `bash`.
pub fn create_bash_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "bash".to_string(),
        label: "bash".to_string(),
        description: format!(
            "Execute a bash command in the current working directory. Returns stdout and stderr. \
             Output is truncated to last {DEFAULT_MAX_OUTPUT_LINES} lines or {}KB (whichever is \
             hit first). If truncated, full output is saved to a temp file. Optionally provide a \
             timeout in seconds.",
            DEFAULT_MAX_OUTPUT_BYTES / 1024
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Bash command to execute"},
                "description": {
                    "type": "string",
                    "description": "Brief present-participle summary of the command's purpose, \
                                    such as 'Running tests' or 'Validating and committing changes'",
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (optional, no default timeout)",
                },
            },
            "required": ["command", "description"],
        }),
        prompt_snippet: Some("Execute bash commands (ls, grep, find, etc.)".to_string()),
        prompt_guidelines: vec![
            "When using bash, include a brief present-participle description of the command's \
             purpose (for example, 'Running tests')."
                .to_string(),
        ],
    }
}

/// Create a `bash` tool.
///
/// The reference threads a cancellation token per call; the Rust `Tool` trait
/// carries none, so the tool holds the token from construction instead.
pub fn create_bash_tool(
    cwd: impl AsRef<Path>,
    shell_command_prefix: Option<&str>,
    cancellation: Option<Arc<dyn CancellationToken>>,
) -> BashTool {
    // The reference is `shell_command_prefix.strip() if shell_command_prefix
    // else None`: an empty string is dropped, but a whitespace-only prefix
    // survives as `""`, which is not `None`. That distinction is observable —
    // the command becomes `"\n{command}"` and `shell_command_prefix_applied`
    // stays `true` — so the emptiness test comes before the trim.
    let prefix = shell_command_prefix
        .filter(|prefix| !prefix.is_empty())
        .map(str::trim)
        .map(str::to_string);
    BashTool {
        spec: create_bash_tool_spec(),
        cwd: cwd.as_ref().to_path_buf(),
        shell_command_prefix: prefix,
        cancellation,
    }
}

/// Apply the opt-in setup prefix the way the reference does.
fn prefixed_shell_command(command: &str, prefix: Option<&str>) -> String {
    match prefix {
        None => command.to_string(),
        Some(prefix) => format!("{prefix}\n{command}"),
    }
}

/// Append command status text after a blank line when output already exists.
fn append_status_block(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_string()
    } else {
        format!("{text}\n\n{status}")
    }
}

/// The exit code the reference would report.
///
/// Python's `returncode` is the negated signal when the process was killed, so a
/// signal death is reported as `-signal` rather than as an absent code.
fn process_exit_code(status: &std::process::ExitStatus) -> Option<i32> {
    if let Some(code) = status.code() {
        return Some(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Some(-signal);
        }
    }
    None
}

/// Write the full, untruncated output to a temporary log file and report it.
///
/// The file deliberately survives the call: the reference opens it with
/// `delete=False`, and it is the artifact the truncation notice points the user
/// at.
fn write_temp_output(output: &str) -> Result<String, ToolError> {
    use std::io::Write;

    let mut file = tempfile::Builder::new()
        .prefix("tau-bash-")
        .suffix(".log")
        .tempfile()
        .map_err(|error| ToolError::new(error.to_string()))?;
    file.write_all(output.as_bytes())
        .and_then(|()| file.flush())
        .map_err(|error| ToolError::new(error.to_string()))?;
    let (_file, path) = file
        .keep()
        .map_err(|error| ToolError::new(error.to_string()))?;
    Ok(path.display().to_string())
}

/// Format a number the way Python's `format(value, "g")` does.
///
/// The reference interpolates the timeout with `{timeout:g}`. Rust has no `g`
/// specifier, so this reproduces Python's default six-significant-digit general
/// format, including the exponent notation for very large and very small values
/// and the stripping of trailing zeros.
fn format_general(value: f64) -> String {
    if !value.is_finite() {
        return format!("{value}");
    }
    if value == 0.0 {
        return "0".to_string();
    }
    let scientific = format!("{value:.5e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("Rust always formats an `f64` in scientific notation with `e`");
    let exponent: i32 = exponent.parse().expect("the exponent is an integer");
    if !(-4..6).contains(&exponent) {
        let mantissa = trim_trailing_zeros(mantissa);
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    } else {
        let decimals = (5 - exponent).max(0) as usize;
        let fixed = format!("{value:.decimals$}");
        trim_trailing_zeros(&fixed).to_string()
    }
}

/// Drop the trailing zeros (and a bare decimal point) of a formatted number.
fn trim_trailing_zeros(number: &str) -> &str {
    if !number.contains('.') {
        return number;
    }
    number.trim_end_matches('0').trim_end_matches('.')
}

/// Whether a configured prefix is truthy, the way the reference tests it.
///
/// The reference takes two decisions out of one prefix and this port keeps them
/// separate. `prefix = shell_command_prefix.strip() if shell_command_prefix else
/// None` turns `"  "` into `""`, which is not `None`, so:
///
/// - `shell_command_prefix_applied` is `prefix is not None`, and the command is
///   `"\n{command}"` — the empty prefix *is* applied.
/// - `executable="bash" if prefix else None` tests truthiness, and `""` is
///   falsy — the command still runs under `/bin/sh`.
fn prefix_is_truthy(prefix: Option<&str>) -> bool {
    prefix.is_some_and(|prefix| !prefix.is_empty())
}

/// Build the platform shell invocation for one command string.
#[cfg(unix)]
fn shell_command(command: &str, prefix_selects_bash: bool) -> tokio::process::Command {
    // The reference runs `/bin/sh -c` and switches the executable to `bash` only
    // when a `shell_command_prefix` is truthy (`executable="bash" if prefix else
    // None`). A whitespace-only prefix strips to `""`, which is falsy, so that
    // command still runs under `/bin/sh` even though the prefix was applied.
    let mut process = if prefix_selects_bash {
        tokio::process::Command::new("bash")
    } else {
        tokio::process::Command::new("sh")
    };
    process.arg("-c").arg(command);
    process
}

/// Build the platform shell invocation for one command string.
#[cfg(windows)]
fn shell_command(command: &str, _prefix_selects_bash: bool) -> tokio::process::Command {
    // `asyncio.create_subprocess_shell` uses `%COMSPEC% /c` on Windows, and the
    // prefix never selects the executable there.
    let mut process = tokio::process::Command::new(shell_program(
        std::env::var_os("COMSPEC"),
        std::env::var_os("SystemRoot"),
    ));
    // CPython builds the shell line as `<comspec> /c "<command>"`
    // (`subprocess.Popen._execute_child`) with no escaping of any kind, and
    // `cmd.exe` follows neither `CommandLineToArgvW` nor a `\"` escape. Rust's
    // argv quoting escapes every embedded `"` as `\"`, which `cmd` echoes back
    // verbatim, so the whole shell argument has to be appended literally.
    // `raw_arg` is that API and exists in the installed tokio 1.53.2
    // (`src/process/mod.rs:420`, `cfg_windows`).
    process.arg("/c").raw_arg(format!("\"{command}\""));
    process
}

/// Start the command as its own process-group leader with a single output pipe.
///
/// Stdin is the null device so interactive programs cannot compete with the
/// agent for terminal input. Stdout and stderr share one endpoint of a socket
/// pair, which reproduces the reference's `stderr=STDOUT`: the child writes both
/// to the same stream, in the order the child produced them. `process_group(0)`
/// is the reference's `start_new_session`.
#[cfg(unix)]
fn spawn_child(
    command: &str,
    prefix_selects_bash: bool,
    cwd: &Path,
) -> std::io::Result<(tokio::process::Child, std::os::unix::net::UnixStream)> {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let (reader, writer) = UnixStream::pair()?;
    let writer_for_stderr = writer.try_clone()?;
    let mut process = shell_command(command, prefix_selects_bash);
    process.current_dir(cwd);
    process.stdin(Stdio::null());
    process.stdout(Stdio::from(OwnedFd::from(writer)));
    process.stderr(Stdio::from(OwnedFd::from(writer_for_stderr)));
    process.process_group(0);
    Ok((process.spawn()?, reader))
}

/// Start the command with the two pipes tokio provides.
///
/// Windows has no `socketpair`, so stdout and stderr stay separate pipes and the
/// two readers feed one channel; the merge is then best-effort rather than
/// strictly ordered. Stdin is the null device, as on the other platform.
#[cfg(windows)]
fn spawn_child(
    command: &str,
    prefix_selects_bash: bool,
    cwd: &Path,
) -> std::io::Result<tokio::process::Child> {
    let mut process = shell_command(command, prefix_selects_bash);
    process.current_dir(cwd);
    process.stdin(Stdio::null());
    process.stdout(Stdio::piped());
    process.stderr(Stdio::piped());
    process.spawn()
}

/// Read the merged output stream off the runtime and forward it as chunks.
///
/// A blocking reader on a dedicated thread keeps the async driver free to poll
/// the cancellation token; the read returns when the child closes the last write
/// end, which the process-group kill guarantees.
#[cfg(unix)]
fn spawn_unix_reader(reader: std::os::unix::net::UnixStream, sender: ChunkSender) {
    use std::io::Read;

    drop(tokio::task::spawn_blocking(move || {
        let mut reader = reader;
        let mut buffer = vec![0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if sender.send(Ok(buffer[..read].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }));
}

/// Forward one of the child's pipes as chunks, in arrival order.
#[cfg(windows)]
fn spawn_async_reader<R>(mut reader: R, sender: ChunkSender)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    use tokio::io::AsyncReadExt;

    drop(tokio::spawn(async move {
        let mut buffer = vec![0u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    if sender.send(Ok(buffer[..read].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }));
}

/// Kill the whole command tree.
///
/// The reference starts the command in a new session and kills the process group
/// so shell children from pipelines or compound commands do not outlive a
/// timeout or a cancellation.
#[cfg(unix)]
fn kill_process_tree(_child: &mut tokio::process::Child, pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    let Ok(raw) = i32::try_from(pid) else {
        return;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else {
        return;
    };
    // `ESRCH` (no such process) is the reference's `ProcessLookupError`: the
    // group already exited, which is not an error here.
    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
}

/// Kill the direct child.
///
/// This is the reference's non-POSIX fallback. Windows has no process-group
/// kill through a safe API here, so a grandchild the shell leaves behind is not
/// reached, exactly as in the reference. `start_kill` initiates the termination;
/// the later `wait` reaps it.
#[cfg(windows)]
fn kill_process_tree(child: &mut tokio::process::Child, _pid: Option<u32>) {
    let _ = child.start_kill();
}

/// The Windows shell `subprocess` would resolve for the reference.
///
/// CPython resolves it in `subprocess.Popen._execute_child`: `comspec =
/// os.environ.get('ComSpec')`; when that is falsy,
/// `os.path.join(os.environ.get('SystemRoot', ''), 'System32', 'cmd.exe')`; and
/// when the result is still not absolute it raises `FileNotFoundError('shell
/// not found: neither %ComSpec% nor %SystemRoot% is set')`. The resolved value
/// is both the executable and the first word of the command line, unescaped.
///
/// The resolution is a pure function of the two environment values so it can be
/// pinned without mutating the process environment, which this workspace
/// forbids. The bare `cmd` last resort is this port's, not the reference's:
/// CPython raises instead of falling back. That is a disclosed deviation rather
/// than a silent one — a bare `cmd` still resolves through the search path, so
/// the tool stays usable on a host where neither variable is set.
#[cfg(windows)]
fn shell_program(
    comspec: Option<std::ffi::OsString>,
    system_root: Option<std::ffi::OsString>,
) -> std::ffi::OsString {
    if let Some(comspec) = comspec.filter(|comspec| !comspec.is_empty()) {
        return comspec;
    }
    let system_root = system_root.unwrap_or_default();
    let resolved = Path::new(&system_root).join("System32").join("cmd.exe");
    if resolved.is_absolute() {
        resolved.into_os_string()
    } else {
        std::ffi::OsString::from("cmd")
    }
}

#[cfg(test)]
mod tests {
    use super::prefix_is_truthy;

    /// `create_bash_tool` stores the reference's `strip()`ed prefix, so a
    /// whitespace-only `"  "` arrives at this decision as `""`: the prefix is
    /// applied, but it does not select `bash`.
    #[test]
    fn a_whitespace_only_prefix_is_applied_but_does_not_select_bash() {
        assert!(!prefix_is_truthy(None));
        // `"  "` after `strip()`.
        assert!(!prefix_is_truthy(Some("")));
        assert!(prefix_is_truthy(Some("export X=1")));
    }

    #[cfg(windows)]
    mod windows {
        use super::super::shell_program;
        use std::ffi::OsString;

        #[test]
        fn windows_shell_follows_cpython_resolution() {
            let comspec = OsString::from(r"C:\Windows\System32\cmd.exe");
            let system_root = OsString::from(r"C:\WINDOWS");
            let resolved = OsString::from(r"C:\WINDOWS\System32\cmd.exe");

            // `ComSpec` wins whenever it is set and non-empty.
            assert_eq!(
                shell_program(Some(comspec.clone()), Some(system_root.clone())),
                comspec
            );
            // A falsy `ComSpec` falls back to `<SystemRoot>\System32\cmd.exe`.
            assert_eq!(
                shell_program(None, Some(system_root.clone())),
                resolved.clone()
            );
            assert_eq!(
                shell_program(Some(OsString::new()), Some(system_root)),
                resolved
            );
            // CPython raises when neither resolves to an absolute path; the
            // bare `cmd` is this port's disclosed last resort.
            assert_eq!(shell_program(None, None), OsString::from("cmd"));
            assert_eq!(
                shell_program(Some(OsString::new()), None),
                OsString::from("cmd")
            );
            assert_eq!(
                shell_program(None, Some(OsString::new())),
                OsString::from("cmd")
            );
        }
    }
}
