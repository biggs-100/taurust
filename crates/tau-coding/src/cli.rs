//! The command-line parse surface.
//!
//! Ported from tau's `tau_coding/cli.py`, which is a single typer callback
//! (`cli.py:192`) plus six hand-dispatched pseudo-commands (`cli.py:448-515`):
//! `install`, `update`, `sessions`, `export`, `providers` and `setup`. tau
//! registers **no** subcommands, so those words are ordinary positional
//! arguments that the callback inspects.
//!
//! What this module owns: option names, short aliases, value kinds, defaults,
//! repetition, hidden flags, the Click-compatible parsing rules, every usage
//! error the reference produces, and the prompt text assembly. What it does
//! **not** own: the behaviour behind any option. The parse surface is portable
//! now; the modules each option needs are not, and each of those is reported by
//! the binary as not implemented yet rather than silently accepted.
//!
//! # Fidelity notes
//!
//! Parsing follows the vendored Click that typer actually runs
//! (`typer/_click/parser.py`), as observed live against tau 0.1.3 / typer 0.27.0:
//!
//! - typer sets `allow_interspersed_args = False` (`typer/core.py:999`), so
//!   option parsing **stops at the first positional**. `tau -p hi --session-id x`
//!   sends `hi --session-id x` as the prompt; the option is only read when it
//!   precedes the prompt;
//! - `--` ends option parsing; everything after it is positional;
//! - a multi-character token with a single dash (`-na`) is a *long* option, so it
//!   is matched whole; a single character with a single dash (`-p`) is short, and
//!   short flags combine (`-pt high`);
//! - a value-taking option consumes the next argument unconditionally, even when
//!   it looks like an option (`-m -x` sets the model to `-x`);
//! - `--long=value` attaches the value, `-m=x` does not (the value becomes `=x`);
//! - `allow_extra_args` with `ignore_unknown_options` (`cli.py:122`) makes an
//!   unknown word or unknown option part of the prompt instead of an error.
//!
//! Error precedence was observed rather than derived. Click's help option is
//! eager, so:
//!
//! 1. structural parse errors (a missing argument, a value on a flag) win over
//!    everything, including `--help`;
//! 2. `--help` wins over value conversion and over every body validation;
//! 3. value conversion runs next, in the order the options appeared on the
//!    command line;
//! 4. the `--version` check runs in the callback body, so it wins over every
//!    other body validation but loses to 1 and 3.
//!
//! Two error renderings exist in the reference. A type or body error is printed
//! under the usage block and `Try 'tau --help' for help.`; a structural parser
//! error is printed alone, because Click only renders usage for an error that
//! carries its context.
//!
//! # Deliberate deviations
//!
//! - `--help` is rendered as plain text. Every string inside it — the usage
//!   line, the option tokens, metavars, help strings, defaults and the epilog —
//!   is the reference's own, and the reference's help tests (whitespace-collapsed
//!   fragments) pass against it, but Click's and Rich's box-drawing layout is not
//!   reproduced. Reproducing that layout byte for byte is a rendering job, and
//!   the reference's own tests treat the layout as incidental.
//! - `USAGE` is extracted from the rendering, so the usage line appears twice in
//!   this file. A test pins them equal.
//! - The numeric readers trim surrounding whitespace, as Python's `int()` and
//!   `float()` do, but do not reproduce Python's accepted underscores
//!   (`int("1_0") == 10`).
//! - `session_manager.validate_session_id` is reproduced here because the CLI
//!   surfaces its messages as usage errors and `tau-coding::session_manager` is
//!   not ported. It must move there, with its four constants, when it is.
//! - `_resolve_prompt_input` / `_resolve_append_system_prompts` — the file-or-
//!   literal resolution of `--system-prompt` and `--append-system-prompt` — are
//!   not ported: they belong to the system-prompt assembly module. The raw
//!   values are carried instead.
//! - The `TauPaths()` construction check that turns a malformed `TAU_HOME` into
//!   a usage error (`cli.py:387-390`) is not performed here. It belongs to
//!   `tau-coding::paths` and needs the environment source, which the parse
//!   surface deliberately does not read.
//! - `_parse_export_cli_args` uses `Path.expanduser()`. Rust's `PathBuf` has no
//!   equivalent without a passwd lookup, so a `~`-prefixed export destination is
//!   passed through unchanged.

use std::collections::VecDeque;
use std::path::PathBuf;

use tau_ai::env::{
    DEFAULT_OPENAI_COMPATIBLE_BASE_URL, DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES,
    DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS, DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS,
};

// The reference reaches this type through `tau_coding.thinking`, which is not
// ported; `provider_config` already carries the vocabulary on the same terms,
// and it must move to `tau-coding::thinking` when that module lands.
pub use crate::provider_config::ThinkingLevel;

use crate::provider_config::{DEFAULT_MODEL, DEFAULT_PROVIDER_NAME, normalize_thinking_level};

/// One declared option, as the parse surface exposes it for inspection.
///
/// The option table is the authoritative registry of the reference's surface;
/// this view lets a test or the parity gate walk it without re-reading
/// `cli.py`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclaredOption {
    /// Every token that names the option, in declaration order. Click
    /// classifies a single dash followed by exactly one character as short and
    /// everything else as long, so `-na` is listed here exactly as
    /// `--no-approve` is.
    pub tokens: &'static [&'static str],
    /// The metavar the help renders, when it shows one.
    pub metavar: Option<&'static str>,
    /// The rendered `[default: …]`, when the help shows one.
    pub default: Option<&'static str>,
    /// The reference's help string, or `None` when `hidden=True` keeps the
    /// option out of `--help`.
    pub help: Option<&'static str>,
}

/// Every declared option, in the callback's declaration order.
///
/// Click folds a `secondary_opt` onto the parameter that owns it —
/// `--no-set-default` belongs to `--set-default`, and the help renders them on
/// one line — so only the owning parameter is reported here, with both tokens.
/// The count therefore matches the reference's 31 declared options.
pub fn declared_options() -> Vec<DeclaredOption> {
    OPTIONS
        .iter()
        .filter_map(|spec| {
            let tokens = spec.rendered_tokens();
            if tokens.is_empty() {
                return None;
            }
            Some(DeclaredOption {
                tokens,
                metavar: spec.metavar,
                default: spec.default,
                help: spec.help,
            })
        })
        .collect()
}

/// The Click usage line, as the reference renders it.
pub const USAGE: &str = "Usage: tau [OPTIONS] [prompt_args]... COMMAND [ARGS]...";

/// The advice line the reference prints under the usage block.
const TRY_HELP: &str = "Try 'tau --help' for help.";

/// The title of the panel the reference prints an error in.
const ERROR_TITLE: &str = "Error";

/// The full `--help` rendering.
///
/// The layout is plain text; every string in it is the reference's own. The
/// option order is the callback's declaration order, and hidden options are
/// absent, exactly as the observed `--help` omits them.
pub const HELP: &str = "\
Usage: tau [OPTIONS] [prompt_args]... COMMAND [ARGS]...

Tau coding-agent harness.

Arguments:
  prompt_args  <str>                      Initial prompt to run in interactive TUI mode.

Options:
  --print  -p                             Run the positional prompt in non-interactive print mode.
  --provider  <str>                       Configured provider name to use.
  --model  -m  <str>                      Model name to request from the provider.
  --thinking  -t  <str>                   Initial thinking level for this run (off, minimal, low, medium, high, xhigh, max). Overrides remembered defaults without persisting.
  --base-url  <str>                       OpenAI-compatible base URL for `tau setup`. [default: https://api.openai.com/v1]
  --api-key-env  <str>                    API key environment variable for `tau setup`. [default: OPENAI_API_KEY]
  --timeout-seconds  <float>              HTTP timeout in seconds for `tau setup` provider requests. [default: 60.0]
  --max-retries  <int>                    Provider retry count for `tau setup`. [default: 2]
  --max-retry-delay-seconds  <float>      Provider retry delay in seconds for `tau setup`. [default: 1.0]
  --set-default  --no-set-default         Make setup provider the default. [default: set-default]
  --cwd  <path>                           Working directory for built-in coding tools.
  --mode  <text|json|transcript|rpc>      Run headlessly with this output format (text, json, transcript, or rpc).
  --session  <str>                        Resume a session id in TUI or print mode.
  --new-session                           Create a new session in TUI mode (default).
  --session-id  <str>                     Set the exact id for the newly created print-mode session.
  --system-prompt  TEXT_OR_PATH           Replace the default system-prompt base with literal text or a UTF-8 file.
  --append-system-prompt  TEXT_OR_PATH    Append literal text or a UTF-8 file to the system prompt (repeatable).
  --auto-compact-threshold  <int>         Automatically compact TUI context above this rough token estimate.
  --extension  -e  <path>                 Load an extension file or directory (repeatable).
  --export                                Export the given session id or JSONL path (mirrors `tau export`).
  --no-extensions                         Disable extension directory discovery (explicit -e paths still load).
  --project-extensions                    Also load trusted project .tau/extensions (additional code opt-in).
  --approve  -a                           Trust protected project inputs for this run.
  --no-approve  -na                       Decline protected project inputs for this run.
  --version  -v                           Show Tau's version and exit.
  --models                                With `tau update`, refresh model catalogs only.
  --help                                  Show this message and exit.

Commands:

  tau install SOURCE [--force] - Install a trusted local or Git extension.

  tau update - Upgrade Tau.

  tau sessions - List indexed sessions.

  tau export REF [DEST] - Export a session as HTML or JSONL.

  tau providers - List configured model providers.

  tau setup - Configure an OpenAI-compatible provider.
";

/// The output format `--mode` selects, from `tau_coding.rendering`.
///
/// Defined here because `tau-coding::rendering` is not ported, on the same
/// terms as the thinking vocabulary in `provider_config`: it moves there when
/// that module lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrintOutputMode {
    /// Plain streaming text.
    Text,
    /// One JSON object per line.
    Json,
    /// The recorded transcript.
    Transcript,
    /// The stdio RPC surface.
    Rpc,
}

impl PrintOutputMode {
    /// Every mode, in the order Click reports them in a choice error.
    pub const ALL: [PrintOutputMode; 4] = [
        PrintOutputMode::Text,
        PrintOutputMode::Json,
        PrintOutputMode::Transcript,
        PrintOutputMode::Rpc,
    ];

    /// The spelling the command line uses.
    pub fn as_str(self) -> &'static str {
        match self {
            PrintOutputMode::Text => "text",
            PrintOutputMode::Json => "json",
            PrintOutputMode::Transcript => "transcript",
            PrintOutputMode::Rpc => "rpc",
        }
    }

    /// Parse one spelling, case-sensitively, as Click's `Choice` does.
    pub fn parse(value: &str) -> Option<Self> {
        PrintOutputMode::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
    }
}

/// The project-trust decision `--approve` / `--no-approve` carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustOverride {
    /// Trust protected project inputs.
    Approve,
    /// Decline protected project inputs.
    Decline,
}

/// Every value the command line carries, with the reference's defaults.
///
/// The field names are the callback's parameter names (`cli.py:193-383`), so a
/// reader can line them up with the reference. `thinking_level` is the one
/// derived field: the reference resolves `--thinking` through
/// `normalize_thinking_level` in the callback body, and that resolution happens
/// in [`dispatch`].
#[derive(Debug, Clone, PartialEq)]
pub struct CliOptions {
    /// Every positional token, in order. The reference's `prompt_args`.
    pub prompt_args: Vec<String>,
    /// `--print` / `-p`.
    pub print_mode: bool,
    /// `--prompt`, hidden: removed in favour of the positional prompt.
    pub prompt_option: Option<String>,
    /// `--provider`.
    pub provider: Option<String>,
    /// `--model` / `-m`.
    pub model: Option<String>,
    /// `--thinking` / `-t`, as typed.
    pub thinking: Option<String>,
    /// The resolved thinking level, when `--thinking` was given.
    pub thinking_level: Option<ThinkingLevel>,
    /// `--base-url`.
    pub setup_base_url: String,
    /// `--api-key-env`.
    pub setup_api_key_env: String,
    /// `--timeout-seconds`.
    pub setup_timeout_seconds: f64,
    /// `--max-retries`.
    pub setup_max_retries: i64,
    /// `--max-retry-delay-seconds`.
    pub setup_max_retry_delay_seconds: f64,
    /// `--set-default` / `--no-set-default`.
    pub setup_default: bool,
    /// `--cwd`.
    pub cwd: Option<PathBuf>,
    /// `--mode`.
    pub mode: Option<PrintOutputMode>,
    /// `--output` / `-o`, hidden: renamed to `--mode`.
    pub output: Option<PrintOutputMode>,
    /// `--session`.
    pub session: Option<String>,
    /// `--resume`, hidden: renamed to `--session`.
    pub resume: Option<String>,
    /// `--new-session`.
    pub new_session: bool,
    /// `--session-id`.
    pub session_id: Option<String>,
    /// `--system-prompt`, unresolved.
    pub system_prompt: Option<String>,
    /// `--append-system-prompt`, in order, unresolved.
    pub append_system_prompt: Vec<String>,
    /// `--auto-compact-threshold`.
    pub auto_compact_threshold: Option<i64>,
    /// `--extension` / `-e`, repeatable.
    pub extension: Vec<PathBuf>,
    /// `-x`, hidden: renamed to `-e`/`--extension`.
    pub extension_legacy: Vec<PathBuf>,
    /// `--export`.
    pub export: bool,
    /// `--no-extensions`.
    pub no_extensions: bool,
    /// `--project-extensions`.
    pub project_extensions: bool,
    /// `--approve` / `-a`.
    pub approve: bool,
    /// `--no-approve` / `-na`.
    pub no_approve: bool,
    /// `--version` / `-v`.
    pub version: bool,
    /// `--models`.
    pub models: bool,
}

impl CliOptions {
    /// The first positional argument, which the reference treats as a possible
    /// pseudo-command name.
    pub fn command(&self) -> Option<&str> {
        self.prompt_args.first().map(String::as_str)
    }

    /// The initial prompt: every positional joined with a single space, exactly
    /// as `" ".join(positional_args)`, or nothing when there are none.
    pub fn initial_prompt(&self) -> Option<String> {
        if self.prompt_args.is_empty() {
            None
        } else {
            Some(self.prompt_args.join(" "))
        }
    }

    /// The trust decision this invocation carries, if any.
    pub fn trust_override(&self) -> Option<TrustOverride> {
        if self.approve {
            Some(TrustOverride::Approve)
        } else if self.no_approve {
            Some(TrustOverride::Decline)
        } else {
            None
        }
    }

    /// Whether `--mode rpc` was requested.
    pub fn rpc_requested(&self) -> bool {
        self.mode == Some(PrintOutputMode::Rpc)
    }

    /// Whether non-interactive output was requested, by `--print` or by any
    /// `--mode` other than `rpc`.
    pub fn print_requested(&self) -> bool {
        self.print_mode || (self.mode.is_some() && !self.rpc_requested())
    }

    /// The output format the run will use; `--mode` wins, `text` is the
    /// fallback even when only `--print` was given.
    pub fn effective_output(&self) -> PrintOutputMode {
        self.mode.unwrap_or(PrintOutputMode::Text)
    }

    /// The `tau setup` inputs, with the reference's `or DEFAULT_*` fallbacks.
    pub fn setup_request(&self) -> SetupRequest {
        SetupRequest {
            provider_name: self
                .provider
                .clone()
                .unwrap_or_else(|| DEFAULT_PROVIDER_NAME.to_string()),
            // `setup_command` applies `.rstrip("/")` itself; the parsed value
            // stays as typed.
            base_url: self.setup_base_url.clone(),
            api_key_env: self.setup_api_key_env.clone(),
            model: self
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            timeout_seconds: self.setup_timeout_seconds,
            max_retries: self.setup_max_retries,
            max_retry_delay_seconds: self.setup_max_retry_delay_seconds,
            set_default: self.setup_default,
        }
    }
}

/// The `tau setup` inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupRequest {
    /// `--provider`, or the reference's `DEFAULT_PROVIDER_NAME`.
    pub provider_name: String,
    /// `--base-url`, as typed.
    pub base_url: String,
    /// `--api-key-env`.
    pub api_key_env: String,
    /// `--model`, or the reference's `DEFAULT_MODEL`.
    pub model: String,
    /// `--timeout-seconds`.
    pub timeout_seconds: f64,
    /// `--max-retries`.
    pub max_retries: i64,
    /// `--max-retry-delay-seconds`.
    pub max_retry_delay_seconds: f64,
    /// `--set-default`.
    pub set_default: bool,
}

/// The `tau export` / `tau --export` inputs, from `_parse_export_cli_args`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    /// The session id or JSONL path the reference resolves first.
    pub session_ref: String,
    /// `[DEST]`, when given.
    pub output_path: Option<PathBuf>,
    /// `--format`, when given; the reference validates it later, against the
    /// session it resolved.
    pub export_format: Option<String>,
}

/// A hand-dispatched pseudo-command.
#[derive(Debug, Clone, PartialEq)]
pub enum PseudoCommand {
    /// `tau install SOURCE [--force]`.
    Install {
        /// The extension source.
        source: String,
        /// Replace an existing installation.
        force: bool,
    },
    /// `tau update [--models]`.
    Update {
        /// Refresh model catalogs only.
        models: bool,
    },
    /// `tau sessions`.
    Sessions,
    /// `tau export REF [DEST] [--format html|jsonl]`, or `--export` with the
    /// same positionals.
    Export(ExportRequest),
    /// `tau providers`.
    Providers,
    /// `tau setup`.
    Setup(SetupRequest),
}

/// The non-interactive request.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintRequest {
    /// The full parsed command line.
    pub options: CliOptions,
    /// The effective output format.
    pub mode: PrintOutputMode,
    /// The positional prompt, joined but **not** merged with stdin: the
    /// reference reads standard input at this point, and [`parse_args`] does no
    /// I/O. Apply [`merge_stdin_prompt`] before running.
    pub prompt: String,
}

/// How one invocation of `tau` was interpreted.
#[derive(Debug, Clone, PartialEq)]
pub enum CliOutcome {
    /// `--version` / `-v`: print the version and exit 0.
    Version,
    /// `--help`: print [`HELP`] and exit 0.
    Help,
    /// A pseudo-command, dispatched by the first positional argument.
    Command(PseudoCommand),
    /// The interactive shell, with everything it needs already parsed.
    Interactive(Box<CliOptions>),
    /// Non-interactive print or RPC mode, with everything it needs parsed.
    Print(Box<PrintRequest>),
    /// A usage error: print it and exit 2.
    Usage(UsageError),
}

/// A Click usage error, with the message the reference prints and exit code 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError {
    /// The message body, without the usage block and without Click's quoting.
    pub message: String,
    /// Whether the reference renders the usage block above the message. It does
    /// for type and body errors and does not for structural parser errors.
    pub usage: bool,
}

impl UsageError {
    /// The exit code of every Click usage error.
    pub fn exit_code(&self) -> u8 {
        2
    }

    /// The usage error a `typer.BadParameter(message)` produces.
    pub fn invalid_value(message: impl Into<String>) -> Self {
        UsageError {
            message: format!("Invalid value: {}", message.into()),
            usage: true,
        }
    }

    /// The conversion error a Click `ParamType` produces:
    /// `Invalid value for '<tokens>': <detail>`.
    pub fn invalid_value_for(hint: &str, detail: &str) -> Self {
        UsageError {
            message: format!("Invalid value for {hint}: {detail}"),
            usage: true,
        }
    }

    /// The structural error for a value-taking option with nothing after it.
    pub fn option_requires_an_argument(name: &str) -> Self {
        UsageError {
            message: format!("Option '{name}' requires an argument."),
            usage: false,
        }
    }

    /// The structural error for a flag given a value.
    pub fn option_does_not_take_a_value(name: &str) -> Self {
        UsageError {
            message: format!("Option '{name}' does not take a value."),
            usage: false,
        }
    }

    /// The plain-text reduction of what the reference writes to stderr.
    ///
    /// The reference prints the usage block and its advice line first, then the
    /// message inside a bordered panel titled `Error`. The strings are the
    /// reference's; the panel borders are not reproduced.
    pub fn render(&self) -> String {
        if self.usage {
            format!("{USAGE}\n{TRY_HELP}\n{ERROR_TITLE}: {}\n", self.message)
        } else {
            format!("{ERROR_TITLE}: {}\n", self.message)
        }
    }
}

/// Interpret the process arguments, without reading anything.
///
/// Argument zero (the program name) must already have been removed.
pub fn parse_args<I, S>(args: I) -> CliOutcome
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let argv: Vec<String> = args
        .into_iter()
        .map(|argument| argument.as_ref().to_string())
        .collect();

    let raw = match Parser::new(argv).run() {
        Ok(raw) => raw,
        Err(error) => return CliOutcome::Usage(error),
    };

    // Click's help option is eager: it runs before any parameter is converted
    // and before the callback body, and it loses only to a structural parse
    // error, which has already returned above.
    if raw.help {
        return CliOutcome::Help;
    }

    match raw.into_options() {
        Ok(options) => dispatch(options),
        Err(error) => CliOutcome::Usage(error),
    }
}

/// Merge piped standard input into a print-mode prompt.
///
/// Mirrors `_merge_stdin_prompt` followed by the reference's check that print
/// mode was given a prompt at all. Pass `None` for `piped` when standard input
/// is a terminal; the reference reads the stream itself, and the parse surface
/// must not perform I/O, so the caller supplies the text it already read.
///
/// # Errors
///
/// The reference's usage error when neither the positionals nor standard input
/// produced any prompt text.
pub fn merge_stdin_prompt(prompt: &str, piped: Option<&str>) -> Result<String, UsageError> {
    let piped = piped.filter(|text| !text.is_empty());
    let merged = match piped {
        Some(piped) if prompt.is_empty() => piped.to_string(),
        Some(piped) => format!("{piped}\n\n{prompt}"),
        None => prompt.to_string(),
    };
    if merged.is_empty() {
        return Err(UsageError::invalid_value(
            "Usage: tau --print \"<prompt>\" (or --mode text|json|transcript \"<prompt>\"); \
             a prompt can also be piped in via stdin",
        ));
    }
    Ok(merged)
}

/// Parse the arguments of `tau export`, from `_parse_export_cli_args`.
///
/// # Errors
///
/// The reference's usage errors, wrapped the way `_run_export_cli` wraps them in
/// `typer.BadParameter`.
pub fn parse_export_args(args: &[String]) -> Result<ExportRequest, UsageError> {
    let usage = || UsageError::invalid_value(EXPORT_USAGE);
    let Some(session_ref) = args.first() else {
        return Err(usage());
    };

    let mut output_path = None;
    let mut export_format = None;
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--format" {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(usage());
            };
            export_format = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--format=") {
            export_format = Some(value.to_string());
        } else if arg.starts_with('-') {
            return Err(UsageError::invalid_value(format!(
                "Unknown export option: {arg}"
            )));
        } else if output_path.is_none() {
            // `Path.expanduser()` has no safe counterpart here; the value is kept
            // as typed.
            output_path = Some(PathBuf::from(arg));
        } else {
            return Err(usage());
        }
        index += 1;
    }

    Ok(ExportRequest {
        session_ref: session_ref.clone(),
        output_path,
        export_format,
    })
}

/// The export usage line, shared by every `tau export` argument error.
const EXPORT_USAGE: &str = "Usage: tau export <session-id-or-jsonl> [--format html|jsonl] [output]";

/// The callback body, in the reference's own order (`cli.py:384-515`).
fn dispatch(mut options: CliOptions) -> CliOutcome {
    // The callback's first act: `if version: echo(f"tau {current_version}")`.
    // It runs after value conversion (which is why a bad value still wins) and
    // before every other body validation.
    if options.version {
        return CliOutcome::Version;
    }

    if options.approve && options.no_approve {
        return CliOutcome::Usage(UsageError::invalid_value(
            "--approve and --no-approve cannot be used together",
        ));
    }

    if let Some(resume) = &options.resume {
        return CliOutcome::Usage(UsageError::invalid_value(format!(
            "--resume was renamed to --session. Use `tau --session {resume}` instead."
        )));
    }

    if options.session.is_some() && options.new_session {
        return CliOutcome::Usage(UsageError::invalid_value(
            "--session and --new-session cannot be used together",
        ));
    }
    if options.session.is_some() && options.session_id.is_some() {
        return CliOutcome::Usage(UsageError::invalid_value(
            "--session and --session-id cannot be used together",
        ));
    }

    if let Some(prompt) = &options.prompt_option {
        return CliOutcome::Usage(UsageError::invalid_value(format!(
            "--prompt was removed. Pass the prompt positionally and use --print, e.g. \
             `tau --print \"{prompt}\"`."
        )));
    }

    if let Some(output) = options.output {
        return CliOutcome::Usage(UsageError::invalid_value(format!(
            "--output was renamed to --mode. Use `tau --mode {}` instead.",
            output.as_str()
        )));
    }

    if !options.extension_legacy.is_empty() {
        return CliOutcome::Usage(UsageError::invalid_value(
            "-x was renamed to -e/--extension.",
        ));
    }

    if let Some(thinking) = options.thinking.clone() {
        match normalize_thinking_level(Some(&thinking)) {
            Ok(level) => options.thinking_level = Some(level),
            Err(error) => return CliOutcome::Usage(UsageError::invalid_value(error.to_string())),
        }
    }

    let rpc_requested = options.rpc_requested();
    let print_requested = options.print_requested();

    if let Some(session_id) = &options.session_id {
        if !print_requested {
            return CliOutcome::Usage(UsageError::invalid_value(
                "--session-id is only supported in print mode",
            ));
        }
        if let Err(message) = validate_session_id(session_id) {
            return CliOutcome::Usage(UsageError::invalid_value(message));
        }
    }

    let command = options.command().map(str::to_string);
    let interactive_branch = !rpc_requested && !print_requested;

    if rpc_requested && options.export {
        return CliOutcome::Usage(UsageError::invalid_value(
            "--export cannot be combined with --mode rpc",
        ));
    }

    if interactive_branch && !options.export && command.as_deref() == Some("update") {
        let positional_models =
            options.prompt_args.len() == 2 && options.prompt_args[1] == "--models";
        if options.prompt_args.len() != 1 && !positional_models {
            return CliOutcome::Usage(UsageError::invalid_value("Usage: tau update [--models]"));
        }
        return CliOutcome::Command(PseudoCommand::Update {
            models: options.models || positional_models,
        });
    }

    if options.models {
        return CliOutcome::Usage(UsageError::invalid_value(
            "--models is only supported with `tau update`",
        ));
    }

    if interactive_branch && !options.export && command.as_deref() == Some("install") {
        return match parse_install_args(&options.prompt_args[1..]) {
            Ok(command) => CliOutcome::Command(command),
            Err(error) => CliOutcome::Usage(error),
        };
    }

    if interactive_branch
        && !options.export
        && command.as_deref() == Some("sessions")
        && options.prompt_args.len() == 1
    {
        return CliOutcome::Command(PseudoCommand::Sessions);
    }

    if interactive_branch && !options.export && command.as_deref() == Some("export") {
        return match parse_export_args(&options.prompt_args[1..]) {
            Ok(request) => CliOutcome::Command(PseudoCommand::Export(request)),
            Err(error) => CliOutcome::Usage(error),
        };
    }

    if options.export && !rpc_requested {
        if print_requested {
            return CliOutcome::Usage(UsageError::invalid_value(
                "--export cannot be combined with --print/--mode.",
            ));
        }
        return match parse_export_args(&options.prompt_args) {
            Ok(request) => CliOutcome::Command(PseudoCommand::Export(request)),
            Err(error) => CliOutcome::Usage(error),
        };
    }

    if interactive_branch
        && command.as_deref() == Some("providers")
        && options.prompt_args.len() == 1
    {
        return CliOutcome::Command(PseudoCommand::Providers);
    }

    if interactive_branch && command.as_deref() == Some("setup") && options.prompt_args.len() == 1 {
        return CliOutcome::Command(PseudoCommand::Setup(options.setup_request()));
    }

    if rpc_requested {
        if options.initial_prompt().is_some() {
            return CliOutcome::Usage(UsageError::invalid_value(
                "RPC mode reads commands from stdin and does not accept a prompt",
            ));
        }
        return CliOutcome::Print(Box::new(PrintRequest {
            options,
            mode: PrintOutputMode::Rpc,
            prompt: String::new(),
        }));
    }

    if !print_requested {
        return CliOutcome::Interactive(Box::new(options));
    }

    let prompt = options.initial_prompt().unwrap_or_default();
    let mode = options.effective_output();
    CliOutcome::Print(Box::new(PrintRequest {
        options,
        mode,
        prompt,
    }))
}

/// Parse `tau install`'s own arguments, from `install_command` (`cli.py:266-290`).
fn parse_install_args(args: &[String]) -> Result<PseudoCommand, UsageError> {
    let mut source: Option<String> = None;
    let mut force = false;
    for arg in args {
        if arg == "--force" {
            force = true;
        } else if arg.starts_with('-') {
            return Err(UsageError::invalid_value(format!(
                "Unknown option for `tau install`: {arg}"
            )));
        } else if source.is_none() {
            source = Some(arg.clone());
        } else {
            return Err(install_usage());
        }
    }
    match source {
        Some(source) => Ok(PseudoCommand::Install { source, force }),
        None => Err(install_usage()),
    }
}

fn install_usage() -> UsageError {
    UsageError::invalid_value("Usage: tau install <source> [--force]")
}

/// `session_manager.validate_session_id`, reproduced until that module lands.
///
/// Returns the reference's message on failure.
fn validate_session_id(session_id: &str) -> Result<(), String> {
    const MAX_SESSION_ID_BYTES: usize = 128;
    const RESERVED_SESSION_IDS: [&str; 2] = ["default", "index"];
    const WINDOWS_RESERVED_STEMS: [&str; 22] = [
        "aux", "con", "nul", "prn", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];

    if !is_session_id_shape(session_id) {
        return Err(
            "Session id must be non-empty, contain only alphanumeric characters, '-', '_', and \
             '.', and start and end with an alphanumeric character"
                .to_string(),
        );
    }
    if session_id.len() > MAX_SESSION_ID_BYTES {
        return Err(format!(
            "Session id must be at most {MAX_SESSION_ID_BYTES} bytes"
        ));
    }
    let normalized = session_id.to_lowercase();
    if RESERVED_SESSION_IDS.contains(&normalized.as_str()) {
        return Err(format!("Session id is reserved: {session_id}"));
    }
    let stem = normalized.split('.').next().unwrap_or_default();
    if WINDOWS_RESERVED_STEMS.contains(&stem) {
        return Err(format!(
            "Session id is not a portable file name: {session_id}"
        ));
    }
    Ok(())
}

/// `^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$`.
fn is_session_id_shape(session_id: &str) -> bool {
    fn allowed(character: char) -> bool {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
    }
    let bytes: Vec<char> = session_id.chars().collect();
    match bytes.first() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    match bytes.len() {
        1 => true,
        _ => {
            let last = bytes.last().expect("non-empty");
            last.is_ascii_alphanumeric() && bytes[1..bytes.len() - 1].iter().all(|c| allowed(*c))
        }
    }
}

/// The value kinds a parsed option can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    /// A string, passed through.
    Str,
    /// A Python `int()`.
    Int,
    /// A Python `float()`.
    Float,
    /// A filesystem path.
    Path,
    /// One of the `--mode` spellings.
    Mode,
}

/// How an option consumes arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arity {
    /// A boolean flag.
    Flag,
    /// One value.
    Value(ValueKind),
    /// One value per occurrence, accumulated in order.
    Append(ValueKind),
}

/// One declared option, in the callback's declaration order.
struct OptionSpec {
    id: Opt,
    /// Every token that names it, in declaration order. For a secondary option
    /// this holds only its own token.
    tokens: &'static [&'static str],
    arity: Arity,
    /// The metavar Click renders, when the help shows one.
    metavar: Option<&'static str>,
    /// `None` mirrors `hidden=True`: the option parses but the help omits it.
    help: Option<&'static str>,
    /// The rendered `[default: …]`, when the help shows one.
    default: Option<&'static str>,
}

impl OptionSpec {
    /// The tokens the help renders for the parameter this option belongs to.
    ///
    /// Click's `--set-default/--no-set-default` is one parameter with two
    /// flags; the parser registers both, the help prints one line. An empty
    /// result marks a spec that is only the secondary half.
    fn rendered_tokens(&self) -> &'static [&'static str] {
        match self.id {
            Opt::SetDefault => &["--set-default", "--no-set-default"],
            Opt::NoSetDefault => &[],
            _ => self.tokens,
        }
    }
}

/// The identity of a declared option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opt {
    Help,
    Print,
    Prompt,
    Provider,
    Model,
    Thinking,
    BaseUrl,
    ApiKeyEnv,
    TimeoutSeconds,
    MaxRetries,
    MaxRetryDelaySeconds,
    SetDefault,
    NoSetDefault,
    Cwd,
    Mode,
    Output,
    Session,
    Resume,
    NewSession,
    SessionId,
    SystemPrompt,
    AppendSystemPrompt,
    AutoCompactThreshold,
    Extension,
    ExtensionLegacy,
    Export,
    NoExtensions,
    ProjectExtensions,
    Approve,
    NoApprove,
    Version,
    Models,
}

/// Every declared option, in the reference's declaration order.
static OPTIONS: [OptionSpec; 32] = [
    OptionSpec {
        id: Opt::Help,
        tokens: &["--help"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Show this message and exit."),
        default: None,
    },
    OptionSpec {
        id: Opt::Print,
        tokens: &["--print", "-p"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Run the positional prompt in non-interactive print mode."),
        default: None,
    },
    OptionSpec {
        id: Opt::Prompt,
        tokens: &["--prompt"],
        arity: Arity::Value(ValueKind::Str),
        metavar: None,
        help: None,
        default: None,
    },
    OptionSpec {
        id: Opt::Provider,
        tokens: &["--provider"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("<str>"),
        help: Some("Configured provider name to use."),
        default: None,
    },
    OptionSpec {
        id: Opt::Model,
        tokens: &["--model", "-m"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("<str>"),
        help: Some("Model name to request from the provider."),
        default: None,
    },
    OptionSpec {
        id: Opt::Thinking,
        tokens: &["--thinking", "-t"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("<str>"),
        help: Some(
            "Initial thinking level for this run (off, minimal, low, medium, high, xhigh, max). \
             Overrides remembered defaults without persisting.",
        ),
        default: None,
    },
    OptionSpec {
        id: Opt::BaseUrl,
        tokens: &["--base-url"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("<str>"),
        help: Some("OpenAI-compatible base URL for `tau setup`."),
        default: Some(DEFAULT_OPENAI_COMPATIBLE_BASE_URL),
    },
    OptionSpec {
        id: Opt::ApiKeyEnv,
        tokens: &["--api-key-env"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("<str>"),
        help: Some("API key environment variable for `tau setup`."),
        default: Some("OPENAI_API_KEY"),
    },
    OptionSpec {
        id: Opt::TimeoutSeconds,
        tokens: &["--timeout-seconds"],
        arity: Arity::Value(ValueKind::Float),
        metavar: Some("<float>"),
        help: Some("HTTP timeout in seconds for `tau setup` provider requests."),
        default: Some("60.0"),
    },
    OptionSpec {
        id: Opt::MaxRetries,
        tokens: &["--max-retries"],
        arity: Arity::Value(ValueKind::Int),
        metavar: Some("<int>"),
        help: Some("Provider retry count for `tau setup`."),
        default: Some("2"),
    },
    OptionSpec {
        id: Opt::MaxRetryDelaySeconds,
        tokens: &["--max-retry-delay-seconds"],
        arity: Arity::Value(ValueKind::Float),
        metavar: Some("<float>"),
        help: Some("Provider retry delay in seconds for `tau setup`."),
        default: Some("1.0"),
    },
    OptionSpec {
        id: Opt::SetDefault,
        tokens: &["--set-default"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Make setup provider the default."),
        default: Some("set-default"),
    },
    OptionSpec {
        id: Opt::NoSetDefault,
        tokens: &["--no-set-default"],
        arity: Arity::Flag,
        metavar: None,
        help: None,
        default: None,
    },
    OptionSpec {
        id: Opt::Cwd,
        tokens: &["--cwd"],
        arity: Arity::Value(ValueKind::Path),
        metavar: Some("<path>"),
        help: Some("Working directory for built-in coding tools."),
        default: None,
    },
    OptionSpec {
        id: Opt::Mode,
        tokens: &["--mode"],
        arity: Arity::Value(ValueKind::Mode),
        metavar: Some("<text|json|transcript|rpc>"),
        help: Some("Run headlessly with this output format (text, json, transcript, or rpc)."),
        default: None,
    },
    OptionSpec {
        id: Opt::Output,
        tokens: &["--output", "-o"],
        arity: Arity::Value(ValueKind::Mode),
        metavar: Some("<text|json|transcript|rpc>"),
        help: None,
        default: None,
    },
    OptionSpec {
        id: Opt::Session,
        tokens: &["--session"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("<str>"),
        help: Some("Resume a session id in TUI or print mode."),
        default: None,
    },
    OptionSpec {
        id: Opt::Resume,
        tokens: &["--resume"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("<str>"),
        help: None,
        default: None,
    },
    OptionSpec {
        id: Opt::NewSession,
        tokens: &["--new-session"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Create a new session in TUI mode (default)."),
        default: None,
    },
    OptionSpec {
        id: Opt::SessionId,
        tokens: &["--session-id"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("<str>"),
        help: Some("Set the exact id for the newly created print-mode session."),
        default: None,
    },
    OptionSpec {
        id: Opt::SystemPrompt,
        tokens: &["--system-prompt"],
        arity: Arity::Value(ValueKind::Str),
        metavar: Some("TEXT_OR_PATH"),
        help: Some("Replace the default system-prompt base with literal text or a UTF-8 file."),
        default: None,
    },
    OptionSpec {
        id: Opt::AppendSystemPrompt,
        tokens: &["--append-system-prompt"],
        arity: Arity::Append(ValueKind::Str),
        metavar: Some("TEXT_OR_PATH"),
        help: Some("Append literal text or a UTF-8 file to the system prompt (repeatable)."),
        default: None,
    },
    OptionSpec {
        id: Opt::AutoCompactThreshold,
        tokens: &["--auto-compact-threshold"],
        arity: Arity::Value(ValueKind::Int),
        metavar: Some("<int>"),
        help: Some("Automatically compact TUI context above this rough token estimate."),
        default: None,
    },
    OptionSpec {
        id: Opt::Extension,
        tokens: &["--extension", "-e"],
        arity: Arity::Append(ValueKind::Path),
        metavar: Some("<path>"),
        help: Some("Load an extension file or directory (repeatable)."),
        default: None,
    },
    OptionSpec {
        id: Opt::ExtensionLegacy,
        tokens: &["-x"],
        arity: Arity::Append(ValueKind::Path),
        metavar: Some("<path>"),
        help: None,
        default: None,
    },
    OptionSpec {
        id: Opt::Export,
        tokens: &["--export"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Export the given session id or JSONL path (mirrors `tau export`)."),
        default: None,
    },
    OptionSpec {
        id: Opt::NoExtensions,
        tokens: &["--no-extensions"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Disable extension directory discovery (explicit -e paths still load)."),
        default: None,
    },
    OptionSpec {
        id: Opt::ProjectExtensions,
        tokens: &["--project-extensions"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Also load trusted project .tau/extensions (additional code opt-in)."),
        default: None,
    },
    OptionSpec {
        id: Opt::Approve,
        tokens: &["--approve", "-a"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Trust protected project inputs for this run."),
        default: None,
    },
    OptionSpec {
        id: Opt::NoApprove,
        tokens: &["--no-approve", "-na"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Decline protected project inputs for this run."),
        default: None,
    },
    OptionSpec {
        id: Opt::Version,
        tokens: &["--version", "-v"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("Show Tau's version and exit."),
        default: None,
    },
    OptionSpec {
        id: Opt::Models,
        tokens: &["--models"],
        arity: Arity::Flag,
        metavar: None,
        help: Some("With `tau update`, refresh model catalogs only."),
        default: None,
    },
];

/// A single-dash, single-character token is a short option; typer's vendored
/// Click classifies everything else as long, including `-na`.
fn is_short_token(token: &str) -> bool {
    token.len() == 2 && token.starts_with('-')
}

fn option_spec(id: Opt) -> &'static OptionSpec {
    OPTIONS
        .iter()
        .find(|spec| spec.id == id)
        .expect("every Opt has a spec")
}

fn find_long(token: &str) -> Option<&'static OptionSpec> {
    if is_short_token(token) {
        return None;
    }
    OPTIONS.iter().find(|spec| spec.tokens.contains(&token))
}

fn find_short(token: &str) -> Option<&'static OptionSpec> {
    OPTIONS.iter().find(|spec| spec.tokens.contains(&token))
}

/// What the parser collected before any value was converted.
#[derive(Debug, Clone, PartialEq)]
struct RawOptions {
    help: bool,
    prompt_args: Vec<String>,
    print_mode: bool,
    prompt_option: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    setup_base_url: String,
    setup_api_key_env: String,
    setup_timeout_seconds: Option<String>,
    setup_max_retries: Option<String>,
    setup_max_retry_delay_seconds: Option<String>,
    setup_default: bool,
    cwd: Option<PathBuf>,
    mode: Option<String>,
    output: Option<String>,
    session: Option<String>,
    resume: Option<String>,
    new_session: bool,
    session_id: Option<String>,
    system_prompt: Option<String>,
    append_system_prompt: Vec<String>,
    auto_compact_threshold: Option<String>,
    extension: Vec<PathBuf>,
    extension_legacy: Vec<PathBuf>,
    export: bool,
    no_extensions: bool,
    project_extensions: bool,
    approve: bool,
    no_approve: bool,
    version: bool,
    models: bool,
    /// Options in the order they were first seen, which is the order Click
    /// converts them in (`iter_params_for_processing`).
    order: Vec<Opt>,
}

impl RawOptions {
    fn new() -> Self {
        RawOptions {
            help: false,
            prompt_args: Vec::new(),
            print_mode: false,
            prompt_option: None,
            provider: None,
            model: None,
            thinking: None,
            setup_base_url: DEFAULT_OPENAI_COMPATIBLE_BASE_URL.to_string(),
            setup_api_key_env: "OPENAI_API_KEY".to_string(),
            setup_timeout_seconds: None,
            setup_max_retries: None,
            setup_max_retry_delay_seconds: None,
            setup_default: true,
            cwd: None,
            mode: None,
            output: None,
            session: None,
            resume: None,
            new_session: false,
            session_id: None,
            system_prompt: None,
            append_system_prompt: Vec::new(),
            auto_compact_threshold: None,
            extension: Vec::new(),
            extension_legacy: Vec::new(),
            export: false,
            no_extensions: false,
            project_extensions: false,
            approve: false,
            no_approve: false,
            version: false,
            models: false,
            order: Vec::new(),
        }
    }

    fn record(&mut self, id: Opt) {
        if !self.order.contains(&id) {
            self.order.push(id);
        }
    }

    fn store(&mut self, id: Opt, value: String) {
        self.record(id);
        match id {
            Opt::Prompt => self.prompt_option = Some(value),
            Opt::Provider => self.provider = Some(value),
            Opt::Model => self.model = Some(value),
            Opt::Thinking => self.thinking = Some(value),
            Opt::BaseUrl => self.setup_base_url = value,
            Opt::ApiKeyEnv => self.setup_api_key_env = value,
            Opt::TimeoutSeconds => self.setup_timeout_seconds = Some(value),
            Opt::MaxRetries => self.setup_max_retries = Some(value),
            Opt::MaxRetryDelaySeconds => self.setup_max_retry_delay_seconds = Some(value),
            Opt::Cwd => self.cwd = Some(PathBuf::from(value)),
            Opt::Mode => self.mode = Some(value),
            Opt::Output => self.output = Some(value),
            Opt::Session => self.session = Some(value),
            Opt::Resume => self.resume = Some(value),
            Opt::SessionId => self.session_id = Some(value),
            Opt::SystemPrompt => self.system_prompt = Some(value),
            Opt::AppendSystemPrompt => self.append_system_prompt.push(value),
            Opt::AutoCompactThreshold => self.auto_compact_threshold = Some(value),
            Opt::Extension => self.extension.push(PathBuf::from(value)),
            Opt::ExtensionLegacy => self.extension_legacy.push(PathBuf::from(value)),
            other => unreachable!("{other:?} does not take a value"),
        }
    }

    fn store_flag(&mut self, id: Opt) {
        self.record(id);
        match id {
            Opt::Help => self.help = true,
            Opt::Print => self.print_mode = true,
            Opt::SetDefault => self.setup_default = true,
            Opt::NoSetDefault => self.setup_default = false,
            Opt::NewSession => self.new_session = true,
            Opt::Export => self.export = true,
            Opt::NoExtensions => self.no_extensions = true,
            Opt::ProjectExtensions => self.project_extensions = true,
            Opt::Approve => self.approve = true,
            Opt::NoApprove => self.no_approve = true,
            Opt::Version => self.version = true,
            Opt::Models => self.models = true,
            other => unreachable!("{other:?} takes a value"),
        }
    }

    /// Convert the raw values into [`CliOptions`], in first-seen order.
    fn into_options(self) -> Result<CliOptions, UsageError> {
        let order = self.order.clone();
        let mut options = CliOptions {
            prompt_args: self.prompt_args.clone(),
            print_mode: self.print_mode,
            prompt_option: self.prompt_option.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            thinking_level: None,
            setup_base_url: self.setup_base_url.clone(),
            setup_api_key_env: self.setup_api_key_env.clone(),
            setup_timeout_seconds: DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS,
            setup_max_retries: DEFAULT_OPENAI_COMPATIBLE_MAX_RETRIES,
            setup_max_retry_delay_seconds: DEFAULT_OPENAI_COMPATIBLE_MAX_RETRY_DELAY_SECONDS,
            setup_default: self.setup_default,
            cwd: self.cwd.clone(),
            mode: None,
            output: None,
            session: self.session.clone(),
            resume: self.resume.clone(),
            new_session: self.new_session,
            session_id: self.session_id.clone(),
            system_prompt: self.system_prompt.clone(),
            append_system_prompt: self.append_system_prompt.clone(),
            auto_compact_threshold: None,
            extension: self.extension.clone(),
            extension_legacy: self.extension_legacy.clone(),
            export: self.export,
            no_extensions: self.no_extensions,
            project_extensions: self.project_extensions,
            approve: self.approve,
            no_approve: self.no_approve,
            version: self.version,
            models: self.models,
        };

        for id in order {
            match id {
                Opt::TimeoutSeconds => {
                    if let Some(raw) = &self.setup_timeout_seconds {
                        options.setup_timeout_seconds = read_float(id, raw)?;
                    }
                }
                Opt::MaxRetries => {
                    if let Some(raw) = &self.setup_max_retries {
                        options.setup_max_retries = read_int(id, raw)?;
                    }
                }
                Opt::MaxRetryDelaySeconds => {
                    if let Some(raw) = &self.setup_max_retry_delay_seconds {
                        options.setup_max_retry_delay_seconds = read_float(id, raw)?;
                    }
                }
                Opt::Mode => {
                    if let Some(raw) = &self.mode {
                        options.mode = Some(read_mode(id, raw)?);
                    }
                }
                Opt::Output => {
                    if let Some(raw) = &self.output {
                        options.output = Some(read_mode(id, raw)?);
                    }
                }
                Opt::AutoCompactThreshold => {
                    if let Some(raw) = &self.auto_compact_threshold {
                        options.auto_compact_threshold = Some(read_int(id, raw)?);
                    }
                }
                _ => {}
            }
        }

        Ok(options)
    }
}

/// The `Invalid value for '<tokens>': …` hint Click builds from every declared
/// token of an option.
fn conversion_hint(id: Opt) -> String {
    option_spec(id)
        .tokens
        .iter()
        .map(|token| format!("'{token}'"))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// `int(value)`, with Python's surrounding-whitespace tolerance.
fn read_int(id: Opt, raw: &str) -> Result<i64, UsageError> {
    raw.trim().parse::<i64>().map_err(|_| {
        UsageError::invalid_value_for(
            &conversion_hint(id),
            &format!("'{raw}' is not a valid int."),
        )
    })
}

/// `float(value)`, with Python's surrounding-whitespace tolerance.
fn read_float(id: Opt, raw: &str) -> Result<f64, UsageError> {
    raw.trim().parse::<f64>().map_err(|_| {
        UsageError::invalid_value_for(
            &conversion_hint(id),
            &format!("'{raw}' is not a valid float."),
        )
    })
}

/// A Click `Choice` over the output modes.
fn read_mode(id: Opt, raw: &str) -> Result<PrintOutputMode, UsageError> {
    PrintOutputMode::parse(raw).ok_or_else(|| {
        let choices = PrintOutputMode::ALL
            .iter()
            .map(|mode| format!("'{}'", mode.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        UsageError::invalid_value_for(
            &conversion_hint(id),
            &format!("'{raw}' is not one of {choices}."),
        )
    })
}

/// The Click-compatible option parser.
struct Parser {
    rargs: VecDeque<String>,
    largs: Vec<String>,
    raw: RawOptions,
}

impl Parser {
    fn new(argv: Vec<String>) -> Self {
        Parser {
            rargs: argv.into(),
            largs: Vec::new(),
            raw: RawOptions::new(),
        }
    }

    fn run(mut self) -> Result<RawOptions, UsageError> {
        // `allow_interspersed_args = False`: the first non-option token ends
        // option parsing, and everything from there on is positional.
        while let Some(arg) = self.rargs.pop_front() {
            if arg == "--" {
                break;
            }
            if arg.len() > 1 && arg.starts_with('-') {
                self.process_opts(&arg)?;
            } else {
                self.rargs.push_front(arg);
                break;
            }
        }

        let mut positional: Vec<String> = std::mem::take(&mut self.largs);
        positional.extend(self.rargs.drain(..));
        self.raw.prompt_args = positional;
        Ok(self.raw)
    }

    fn process_opts(&mut self, arg: &str) -> Result<(), UsageError> {
        let (token, explicit_value) = match arg.find('=') {
            Some(index) => (&arg[..index], Some(&arg[index + 1..])),
            None => (arg, None),
        };

        if let Some(spec) = find_long(token) {
            return self.apply_long(spec, token, explicit_value);
        }
        if token.starts_with("--") {
            // `ignore_unknown_options`: an unknown long option becomes a
            // positional instead of an error.
            self.largs.push(arg.to_string());
            return Ok(());
        }
        self.apply_short(arg)
    }

    fn apply_long(
        &mut self,
        spec: &'static OptionSpec,
        token: &str,
        explicit_value: Option<&str>,
    ) -> Result<(), UsageError> {
        match spec.arity {
            Arity::Flag => {
                if explicit_value.is_some() {
                    return Err(UsageError::option_does_not_take_a_value(token));
                }
                self.raw.store_flag(spec.id);
                Ok(())
            }
            Arity::Value(_) | Arity::Append(_) => {
                if let Some(value) = explicit_value {
                    self.rargs.push_front(value.to_string());
                }
                let value = self.take_value(token)?;
                self.raw.store(spec.id, value);
                Ok(())
            }
        }
    }

    fn apply_short(&mut self, arg: &str) -> Result<(), UsageError> {
        let characters: Vec<char> = arg.chars().collect();
        let prefix = characters[0];
        let mut unknown: Vec<char> = Vec::new();
        let mut index = 1;
        let mut stop = false;

        for character in characters[1..].iter() {
            let token = format!("-{character}");
            index += 1;
            let Some(spec) = find_short(&token) else {
                unknown.push(*character);
                continue;
            };
            match spec.arity {
                Arity::Flag => self.raw.store_flag(spec.id),
                Arity::Value(_) | Arity::Append(_) => {
                    if index < characters.len() {
                        // Characters left in the token are the value.
                        self.rargs
                            .push_front(characters[index..].iter().collect::<String>());
                        stop = true;
                    }
                    let value = self.take_value(&token)?;
                    self.raw.store(spec.id, value);
                }
            }
            if stop {
                break;
            }
        }

        if !unknown.is_empty() {
            // Unknown short flags are recombined into one ignored token, which
            // keeps `-V` and `-h` intact as prompt text.
            let ignored: String = unknown.into_iter().collect();
            self.largs.push(format!("{prefix}{ignored}"));
        }
        Ok(())
    }

    fn take_value(&mut self, token: &str) -> Result<String, UsageError> {
        self.rargs
            .pop_front()
            .ok_or_else(|| UsageError::option_requires_an_argument(token))
    }
}
