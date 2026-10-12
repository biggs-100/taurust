//! `tau-agent` — the portable agent brain.
//!
//! Owns messages, tools, events, the agent loop, the harness and the session
//! primitives. It is reusable and terminal-agnostic.
//!
//! Ported from tau's `tau_agent` package (16 modules, ~2.4k LOC).

#![forbid(unsafe_code)]

pub mod agent_loop;
pub mod harness;
pub mod registry;
pub mod session;
pub mod tool_history;
pub mod tools;

pub use async_trait::async_trait;
