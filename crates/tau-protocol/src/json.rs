//! The JSON value vocabulary used by tool schemas, tool arguments and details.

pub use serde_json::Value as JsonValue;

/// A JSON object with string keys.
pub type JsonObject = serde_json::Map<String, JsonValue>;
