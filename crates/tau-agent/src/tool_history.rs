//! Provider-safe repair of a transcript whose tool calls and results do not line up.
//!
//! Ported from tau's `tau_agent/tool_history.py`. A provider rejects a transcript
//! where a tool call has no result, a result has no call, or the two are not
//! adjacent, so the history is normalized before every request.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use tau_protocol::content::{TextContent, ToolCall};
use tau_protocol::message::{AgentMessage, ToolResultMessage};

/// The text of the result synthesized for a tool call that never returned.
pub const INTERRUPTED_TOOL_RESULT: &str = "Tool call interrupted by user";

/// A provider-safe transcript plus a summary of the deterministic repairs.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolHistoryRepair {
    /// The repaired transcript.
    pub messages: Vec<AgentMessage>,
    /// Whether the repair changed anything.
    pub changed: bool,
    /// How many results were synthesized for calls that never returned.
    pub synthesized_results: usize,
    /// How many results were dropped because no call asked for them.
    pub dropped_orphan_results: usize,
    /// How many surplus results were dropped for a call that already had one.
    pub dropped_duplicate_results: usize,
    /// How many results were moved to sit beside their call.
    pub reordered_results: usize,
}

impl ToolHistoryRepair {
    /// JSON-safe counters for durable session diagnostics.
    pub fn diagnostic_data(&self) -> Value {
        json!({
            "synthesizedResults": self.synthesized_results,
            "droppedOrphanResults": self.dropped_orphan_results,
            "droppedDuplicateResults": self.dropped_duplicate_results,
            "reorderedResults": self.reordered_results,
        })
    }
}

/// Return history where every tool call has exactly one adjacent result.
///
/// Existing results are moved beside their calls. A missing result gets a
/// deterministic interruption error. A result with no call is omitted, because
/// the arguments of a call that is not there cannot be reconstructed safely.
/// When the same id has several results, a real one is preferred over the
/// synthesized interruption.
pub fn repair_tool_history(messages: &[AgentMessage]) -> ToolHistoryRepair {
    let mut occurrences: Vec<((usize, usize), ToolCall, usize)> = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let AgentMessage::Assistant(assistant) = message else {
            continue;
        };
        for (offset, call) in assistant.tool_calls().into_iter().enumerate() {
            let call_offset = offset + 1;
            occurrences.push((
                (message_index, call_offset),
                call.clone(),
                message_index + call_offset,
            ));
        }
    }

    let mut results_by_id: HashMap<String, Vec<(usize, ToolResultMessage)>> = HashMap::new();
    for (message_index, message) in messages.iter().enumerate() {
        if let AgentMessage::ToolResult(result) = message {
            results_by_id
                .entry(result.tool_call_id.clone())
                .or_default()
                .push((message_index, result.clone()));
        }
    }

    let mut selected: HashMap<(usize, usize), (Option<usize>, ToolResultMessage)> = HashMap::new();
    let mut used: HashSet<usize> = HashSet::new();

    // Reserve the pairs that are already adjacent first, so a valid repeated id
    // is paired with its own turn instead of being consumed by an earlier one.
    for (occurrence, call, expected) in &occurrences {
        if *expected >= messages.len() {
            continue;
        }
        let AgentMessage::ToolResult(candidate) = &messages[*expected] else {
            continue;
        };
        if candidate.tool_call_id == call.id && !used.contains(expected) {
            selected.insert(*occurrence, (Some(*expected), candidate.clone()));
            used.insert(*expected);
        }
    }

    let mut synthesized_results = 0usize;
    for (occurrence, call, _expected) in &occurrences {
        if selected.contains_key(occurrence) {
            continue;
        }

        let candidates: Vec<(usize, ToolResultMessage)> = results_by_id
            .get(&call.id)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(position, _)| !used.contains(position))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        if candidates.is_empty() {
            selected.insert(*occurrence, (None, interrupted_result(call)));
            synthesized_results += 1;
            continue;
        }

        let after: Vec<(usize, ToolResultMessage)> = candidates
            .iter()
            .filter(|(position, _)| *position > occurrence.0)
            .cloned()
            .collect();
        let pool = if after.is_empty() { candidates } else { after };
        let chosen = pool
            .iter()
            .find(|(_, result)| !is_interruption_result(result))
            .unwrap_or(&pool[0])
            .clone();

        selected.insert(*occurrence, (Some(chosen.0), chosen.1));
        used.insert(chosen.0);
    }

    // When every call is already paired and a real result is left over, prefer it
    // over a synthesized interruption for the same id.
    for (occurrence, call, _expected) in &occurrences {
        let Some((Some(selected_position), selected_result)) = selected.get(occurrence).cloned()
        else {
            continue;
        };
        if !is_interruption_result(&selected_result) {
            continue;
        }

        let replacement = results_by_id.get(&call.id).and_then(|entries| {
            entries
                .iter()
                .find(|(position, result)| {
                    !used.contains(position) && !is_interruption_result(result)
                })
                .cloned()
        });
        let Some((replacement_position, replacement_result)) = replacement else {
            continue;
        };

        used.remove(&selected_position);
        used.insert(replacement_position);
        selected.insert(
            *occurrence,
            (Some(replacement_position), replacement_result),
        );
    }

    let mut repaired: Vec<AgentMessage> = Vec::new();
    let mut reordered_results = 0usize;
    for (message_index, message) in messages.iter().enumerate() {
        if matches!(message, AgentMessage::ToolResult(_)) {
            continue;
        }
        repaired.push(message.clone());

        let AgentMessage::Assistant(assistant) = message else {
            continue;
        };
        for (offset, _call) in assistant.tool_calls().into_iter().enumerate() {
            let call_offset = offset + 1;
            let (result_position, result) = selected
                .get(&(message_index, call_offset))
                .expect("every call occurrence was selected above")
                .clone();
            repaired.push(AgentMessage::ToolResult(result));
            if let Some(position) = result_position
                && position != message_index + call_offset
            {
                reordered_results += 1;
            }
        }
    }

    let call_ids: HashSet<&str> = occurrences
        .iter()
        .map(|(_, call, _)| call.id.as_str())
        .collect();
    let unused: Vec<&ToolResultMessage> = results_by_id
        .values()
        .flatten()
        .filter(|(position, _)| !used.contains(position))
        .map(|(_, result)| result)
        .collect();

    let dropped_orphan_results = unused
        .iter()
        .filter(|result| !call_ids.contains(result.tool_call_id.as_str()))
        .count();
    let dropped_duplicate_results = unused
        .iter()
        .filter(|result| call_ids.contains(result.tool_call_id.as_str()))
        .count();

    let changed = repaired.as_slice() != messages;

    ToolHistoryRepair {
        messages: repaired,
        changed,
        synthesized_results,
        dropped_orphan_results,
        dropped_duplicate_results,
        reordered_results,
    }
}

fn interrupted_result(call: &ToolCall) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content: vec![
            TextContent {
                text: INTERRUPTED_TOOL_RESULT.to_string(),
                text_signature: None,
            }
            .into(),
        ],
        is_error: true,
        ..ToolResultMessage::default()
    }
}

fn is_interruption_result(result: &ToolResultMessage) -> bool {
    result.is_error && result.text() == INTERRUPTED_TOOL_RESULT
}
