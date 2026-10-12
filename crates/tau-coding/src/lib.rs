//! `tau-coding` — the coding application built on the agent brain.
//!
//! Owns the command-line surface, configuration, credentials, the built-in
//! tools, resources, skills and on-disk sessions. The terminal frontend lives
//! in `tau-tui` and consumes the events produced here.
//!
//! Ported from tau's `tau_coding` package (82 modules, ~50k LOC).

#![forbid(unsafe_code)]

pub mod credentials;
pub mod paths;
pub mod provider_config;
pub mod tools;

/// The version of the taurust coding application.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Usage string for the command line.
pub const USAGE: &str = "\
usage: tau [--version | --help]

A small terminal coding agent, in Rust.

With no arguments, tau starts the interactive shell.";

/// How the process arguments were interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliOutcome {
    /// Report the application version and exit successfully.
    Version(String),
    /// Print the usage string and exit successfully.
    Help,
    /// Start the interactive shell.
    StartInteractive,
    /// The argument is not part of the command surface yet.
    Unsupported(String),
}

/// Interpret the process arguments.
///
/// Only arguments that already have real behaviour are recognised; anything
/// else is reported as [`CliOutcome::Unsupported`] rather than silently
/// ignored, so an unimplemented surface never looks like it worked.
pub fn parse_args<I, S>(args: I) -> CliOutcome
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let Some(first) = args.into_iter().next() else {
        return CliOutcome::StartInteractive;
    };

    match first.as_ref() {
        "--version" | "-V" => CliOutcome::Version(VERSION.to_string()),
        "--help" | "-h" => CliOutcome::Help,
        other => CliOutcome::Unsupported(other.to_string()),
    }
}
