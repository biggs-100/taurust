//! Append-only session storage.
//!
//! Ported from tau's `tau_agent/session/` package. The entry model and the JSONL
//! format are here; the file-backed storage with cross-process locking arrives
//! separately.

pub mod entries;
pub mod jsonl;

pub use entries::{EntryBody, SessionEntry, current_timestamp, new_entry_id};
pub use jsonl::{
    SessionJsonlError, entries_from_json_lines, entry_from_json_line, entry_to_json_line,
};
