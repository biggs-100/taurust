//! Built-in filesystem and shell tools for coding sessions.
//!
//! Ported from tau's `tau_coding/tools.py`. Every tool is provider-neutral: each
//! one declares a `ToolSpec` the model sees and implements `tau_agent::tools::Tool`.
//! Relative paths resolve against a configurable working directory, and all local
//! filesystem and shell behaviour stays outside the reusable `tau-agent` package.
//!
//! The reference is one 1200-line module that returns closures. Rust has no need
//! to reproduce that shape: the tools are structs, so the module is split one
//! file per tool plus [`support`], which holds the output and argument vocabulary
//! every tool shares.
//!
//! This slice carries that vocabulary and the image pipeline the `read` tool
//! needs. The tools themselves follow.

pub mod image_processing;
pub mod support;

pub use image_processing::{ImageLimits, ImageProcessingFailure, ProcessedImage, process_image};
