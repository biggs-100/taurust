//! Built-in filesystem and shell tools for coding sessions.
//!
//! Ported from tau's `tau_coding/tools.py`. Every tool is provider-neutral: each
//! one declares a [`ToolSpec`] the model sees and implements
//! [`tau_agent::tools::Tool`]. Relative paths resolve against a configurable
//! working directory, and all local filesystem and shell behaviour stays
//! outside the reusable `tau-agent` package.
//!
//! The reference is one 1200-line module that returns closures. Rust has no need
//! to reproduce that shape: the tools are structs, so the module is split one
//! file per tool plus [`support`], which holds the output and argument
//! vocabulary every tool shares.
//!
//! The reference also threads a per-call `signal` through the executor. The
//! Rust [`tau_agent::tools::Tool`] trait carries no per-call signal, so a tool
//! that needs one (only `bash` does) holds it from construction instead. See
//! [`bash::create_bash_tool`].

use std::path::Path;

use tau_agent::tools::Tool;

pub mod bash;
pub mod diff;
pub mod edit;
pub mod image_processing;
pub mod read;
pub mod support;
pub mod write;

pub use bash::{BashTool, create_bash_tool};
pub use edit::{EditTool, create_edit_tool};
pub use image_processing::{ImageLimits, ImageProcessingFailure, ProcessedImage, process_image};
pub use read::{
    ImageSupportState, LocalReadOperations, ReadOperations, ReadTool, create_read_tool,
};
pub use write::{WriteTool, create_write_tool};

/// Create the default coding-tool set for a local project.
///
/// The tools are ordered as the reference orders them: `read`, `write`, `edit`
/// and `bash`. Relative paths used with those tools resolve against `cwd`. When
/// configured, `shell_command_prefix` is prepended to every bash command.
pub fn create_coding_tools(
    cwd: impl AsRef<Path>,
    shell_command_prefix: Option<&str>,
    image_support: Option<ImageSupportState>,
) -> Vec<Box<dyn Tool>> {
    let root = cwd.as_ref();
    vec![
        Box::new(create_read_tool(root, None, image_support)),
        Box::new(create_write_tool(root)),
        Box::new(create_edit_tool(root)),
        Box::new(create_bash_tool(root, shell_command_prefix, None)),
    ]
}
