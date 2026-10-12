//! `tau-agent` — the portable agent brain.
//!
//! Owns messages, tools, events, the agent loop, the harness and the session
//! primitives. It is reusable and terminal-agnostic.
//!
//! Ported from tau's `tau_agent` package (16 modules, ~2.4k LOC).

#![forbid(unsafe_code)]

pub mod registry;
pub mod tool_history;
pub mod tools;

pub use async_trait::async_trait;
