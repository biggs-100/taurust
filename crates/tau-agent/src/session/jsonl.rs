//! JSONL serialization for persisted sessions.
//!
//! Ported from tau's `tau_agent/session/jsonl.py`.
//!
//! **The persisted shape is not the provider wire shape.** An entry is written
//! with the reference's `exclude_none=True`, so a `null` model field is omitted;
//! the same field is written as `null` when a message is sent to a provider. This
//! module therefore post-processes the serialized value instead of leaning on
//! `serde` attributes that would change the provider format too.
//!
//! Only model fields are dropped: a `null` a caller put inside a free-form
//! payload such as `details` survives, because the reference only excludes model
//! fields and not the contents of arbitrary JSON.
//!
//! Migration of pre-bookmark Tau-v1 session records is not ported here; see the
//! task document.

use std::fmt;

use serde_json::Value;

use super::entries::SessionEntry;

/// A session JSONL line that could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionJsonlError {
    message: String,
}

impl SessionJsonlError {
    /// An error described by `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SessionJsonlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SessionJsonlError {}

/// Serialize one entry in the canonical persisted shape, including its newline.
pub fn entry_to_json_line(entry: &SessionEntry) -> Result<String, SessionJsonlError> {
    let mut value =
        serde_json::to_value(entry).map_err(|error| SessionJsonlError::new(error.to_string()))?;
    drop_model_nulls(&mut value);
    drop_empty_replaced_ids(&mut value);

    let mut line =
        serde_json::to_string(&value).map_err(|error| SessionJsonlError::new(error.to_string()))?;
    line.push('\n');
    Ok(line)
}

/// Deserialize one entry.
pub fn entry_from_json_line(
    line: &str,
    line_number: Option<usize>,
) -> Result<SessionEntry, SessionJsonlError> {
    let location = match line_number {
        Some(number) => format!(" on line {number}"),
        None => String::new(),
    };

    let value: Value = serde_json::from_str(line).map_err(|error| {
        SessionJsonlError::new(format!("Invalid session entry{location}: {error}"))
    })?;

    serde_json::from_value(value).map_err(|error| {
        SessionJsonlError::new(format!("Invalid session entry{location}: {error}"))
    })
}

/// Deserialize the non-empty lines, in order.
pub fn entries_from_json_lines<I, S>(lines: I) -> Result<Vec<SessionEntry>, SessionJsonlError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut entries = Vec::new();
    for (index, line) in lines.into_iter().enumerate() {
        let line = line.as_ref();
        if line.trim().is_empty() {
            continue;
        }
        entries.push(entry_from_json_line(line, Some(index + 1))?);
    }
    Ok(entries)
}

/// Drop the `null` fields of every model object inside an entry.
fn drop_model_nulls(value: &mut Value) {
    let Some(entry) = value.as_object_mut() else {
        return;
    };
    drop_null_fields(entry);

    if let Some(usage) = entry.get_mut("usage").and_then(Value::as_object_mut) {
        drop_null_fields(usage);
        drop_null_fields_of(usage, "cost");
    }

    drop_content_nulls(entry.get_mut("content"));

    let Some(message) = entry.get_mut("message").and_then(Value::as_object_mut) else {
        return;
    };
    drop_null_fields(message);
    for key in ["usage", "timing"] {
        if let Some(object) = message.get_mut(key).and_then(Value::as_object_mut) {
            drop_null_fields(object);
        }
    }
    drop_null_fields_of(message, "cost");
    drop_content_nulls(message.get_mut("content"));
    drop_content_nulls(message.get_mut("diagnostics"));
}

fn drop_null_fields_of(object: &mut serde_json::Map<String, Value>, key: &str) {
    if let Some(nested) = object.get_mut(key).and_then(Value::as_object_mut) {
        drop_null_fields(nested);
    }
}

fn drop_content_nulls(content: Option<&mut Value>) {
    let Some(blocks) = content.and_then(Value::as_array_mut) else {
        return;
    };
    for block in blocks {
        if let Some(object) = block.as_object_mut() {
            drop_null_fields(object);
        }
    }
}

fn drop_null_fields(object: &mut serde_json::Map<String, Value>) {
    object.retain(|_, value| !value.is_null());
}

/// Remove the empty compatibility field the serializer never writes.
fn drop_empty_replaced_ids(value: &mut Value) {
    let Some(entry) = value.as_object_mut() else {
        return;
    };
    if entry
        .get("replaces_entry_ids")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        entry.remove("replaces_entry_ids");
    }
}
