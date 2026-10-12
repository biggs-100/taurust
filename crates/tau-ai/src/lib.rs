//! `tau-ai` — provider-neutral model streaming.
//!
//! Translates concrete model providers into a single provider-neutral event
//! stream. This layer must not know about the terminal, local configuration
//! paths, or rendering.
//!
//! Ported from tau's `tau_ai` package (20 modules, ~5.5k LOC).
