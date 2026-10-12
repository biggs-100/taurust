//! Append-only session entries.
//!
//! Ported from tau's `tau_agent/session/entries.py`. Field names stay snake_case
//! on the wire — unlike transcript messages, which are camelCase — because the
//! reference's entry models have no alias generator. An entry id is a UUID with
//! its dashes removed, and an entry timestamp is seconds with sub-second
//! precision, where a message timestamp is integer milliseconds.

use serde::{Deserialize, Serialize};

use tau_protocol::message::{AgentMessage, Usage};
use tau_protocol::{JsonObject, JsonValue, UserContent};

/// A unique session entry id.
pub fn new_entry_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The current Unix timestamp, in seconds with sub-second precision.
pub fn current_timestamp() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

fn default_true() -> bool {
    true
}

/// The type-specific body of a session entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EntryBody {
    /// A transcript message.
    #[serde(rename = "message")]
    Message {
        /// The message.
        message: AgentMessage,
    },
    /// A message an extension injected that participates in model context.
    #[serde(rename = "custom_message")]
    CustomMessage {
        /// The extension-defined kind.
        custom_type: String,
        /// The message content.
        content: UserContent,
        /// Whether a frontend should render it.
        #[serde(default = "default_true")]
        display: bool,
        /// Structured details for frontends.
        #[serde(default)]
        details: Option<JsonValue>,
    },
    /// A model selection change.
    #[serde(rename = "model_change")]
    ModelChange {
        /// The selected model.
        model: String,
        /// The selected provider.
        ///
        /// Added after the first session format and kept optional so older
        /// transcripts still load.
        #[serde(default)]
        provider: Option<String>,
    },
    /// A thinking level change.
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange {
        /// The selected level.
        #[serde(default)]
        thinking_level: Option<String>,
    },
    /// A context summary that replaces older message entries during replay.
    #[serde(rename = "compaction")]
    Compaction {
        /// The summary.
        summary: String,
        /// The replaced ids.
        ///
        /// Legacy sessions stored every replaced id; new entries normally leave
        /// this empty and the serializer omits it.
        #[serde(default)]
        replaces_entry_ids: Vec<String>,
        /// The first entry kept after compaction.
        #[serde(default)]
        first_kept_entry_id: Option<String>,
        /// The token count before compaction.
        #[serde(default)]
        tokens_before: Option<u64>,
        /// The usage the summary cost.
        #[serde(default)]
        usage: Option<Usage>,
        /// The provider that produced the summary.
        #[serde(default)]
        provider: Option<String>,
        /// The model that produced the summary.
        #[serde(default)]
        model: Option<String>,
        /// The provider that actually served the summary request.
        #[serde(default)]
        response_provider: Option<String>,
    },
    /// A summary of a branch that was left behind.
    #[serde(rename = "branch_summary")]
    BranchSummary {
        /// The summary.
        summary: String,
        /// The id the branch started from.
        #[serde(default)]
        branch_root_id: Option<String>,
        /// The usage the summary cost.
        #[serde(default)]
        usage: Option<Usage>,
        /// The provider that produced the summary.
        #[serde(default)]
        provider: Option<String>,
        /// The model that produced the summary.
        #[serde(default)]
        model: Option<String>,
        /// The provider that actually served the summary request.
        #[serde(default)]
        response_provider: Option<String>,
    },
    /// A bookmark change attached to one entry.
    #[serde(rename = "label")]
    Label {
        /// The entry the label is attached to.
        target_id: String,
        /// The label, or `None` to clear it.
        #[serde(default)]
        label: Option<String>,
    },
    /// A legacy active-tip pointer, retained only so old transcripts load.
    #[serde(rename = "leaf")]
    Leaf {
        /// The active tip.
        #[serde(default)]
        entry_id: Option<String>,
    },
    /// Basic session metadata.
    #[serde(rename = "session_info")]
    SessionInfo {
        /// When the session was created.
        #[serde(default = "current_timestamp")]
        created_at: f64,
        /// The working directory the session started in.
        #[serde(default)]
        cwd: Option<String>,
        /// The session title.
        #[serde(default)]
        title: Option<String>,
    },
    /// Extension- or application-owned data.
    #[serde(rename = "custom")]
    Custom {
        /// The namespace the data belongs to.
        namespace: String,
        /// The data.
        #[serde(default)]
        data: JsonObject,
    },
}

impl EntryBody {
    /// The wire `type` of the body.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Message { .. } => "message",
            Self::CustomMessage { .. } => "custom_message",
            Self::ModelChange { .. } => "model_change",
            Self::ThinkingLevelChange { .. } => "thinking_level_change",
            Self::Compaction { .. } => "compaction",
            Self::BranchSummary { .. } => "branch_summary",
            Self::Label { .. } => "label",
            Self::Leaf { .. } => "leaf",
            Self::SessionInfo { .. } => "session_info",
            Self::Custom { .. } => "custom",
        }
    }
}

/// One append-only session record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    /// The entry id.
    #[serde(default = "new_entry_id")]
    pub id: String,
    /// The entry this one follows, when it is not the first of its branch.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// When the entry was written, in seconds.
    #[serde(default = "current_timestamp")]
    pub timestamp: f64,
    /// The type-specific body.
    #[serde(flatten)]
    pub body: EntryBody,
}

impl SessionEntry {
    /// A new entry with a fresh id and the current time.
    pub fn new(body: EntryBody) -> Self {
        Self {
            id: new_entry_id(),
            parent_id: None,
            timestamp: current_timestamp(),
            body,
        }
    }

    /// An entry with everything stated explicitly, which is what tests want.
    pub fn explicit(id: impl Into<String>, timestamp: f64, body: EntryBody) -> Self {
        Self {
            id: id.into(),
            parent_id: None,
            timestamp,
            body,
        }
    }

    /// The wire `type` of the entry.
    pub fn kind(&self) -> &'static str {
        self.body.kind()
    }

    /// The transcript message, when this is a message entry.
    pub fn message(&self) -> Option<&AgentMessage> {
        match &self.body {
            EntryBody::Message { message } => Some(message),
            _ => None,
        }
    }
}
