//! `tau-coding` — the coding application built on the agent brain.
//!
//! Owns the command-line surface, configuration, credentials, the built-in
//! tools, resources, skills and on-disk sessions. The terminal frontend lives
//! in `tau-tui` and consumes the events produced here.
//!
//! Ported from tau's `tau_coding` package (82 modules, ~50k LOC).

#![forbid(unsafe_code)]

pub mod cli;
pub mod credentials;
pub mod paths;
pub mod provider_config;
pub mod tools;

pub use cli::{
    CliOptions, CliOutcome, ExportRequest, HELP, PrintOutputMode, PrintRequest, PseudoCommand,
    SetupRequest, USAGE, UsageError, parse_args,
};

/// The version of the taurust coding application.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
