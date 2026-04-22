//! Client → Daemon messages.
//!
//! Every request carries an `id` so the daemon's `response.ok` /
//! `response.error` / `session.list.result` can be correlated back to the
//! caller.

use serde::{Deserialize, Serialize};

use crate::session::SessionMode;

/// Tagged union of every message a client can send the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    #[serde(rename = "session.create")]
    SessionCreate {
        id: String,
        name: String,
        workdir: String,
    },

    #[serde(rename = "session.list")]
    SessionList { id: String },

    #[serde(rename = "session.attach")]
    SessionAttach { id: String, session_id: String },

    #[serde(rename = "session.detach")]
    SessionDetach { id: String, session_id: String },

    #[serde(rename = "session.send")]
    SessionSend {
        id: String,
        session_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<Attachment>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        priority: Option<SendPriority>,
    },

    #[serde(rename = "session.interrupt")]
    SessionInterrupt { id: String, session_id: String },

    #[serde(rename = "session.approve")]
    SessionApprove {
        id: String,
        session_id: String,
        approval_id: String,
        approved: bool,
    },

    #[serde(rename = "session.destroy")]
    SessionDestroy { id: String, session_id: String },

    #[serde(rename = "session.set_mode")]
    SessionSetMode {
        id: String,
        session_id: String,
        mode: SessionMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_turns: Option<u32>,
    },

    #[serde(rename = "session.pin")]
    SessionPin {
        id: String,
        session_id: String,
        path: String,
    },

    #[serde(rename = "session.unpin")]
    SessionUnpin {
        id: String,
        session_id: String,
        path: String,
    },

    #[serde(rename = "session.rotate")]
    SessionRotate { id: String, session_id: String },

    #[serde(rename = "session.search")]
    SessionSearch {
        id: String,
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<SearchScope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },

    #[serde(rename = "session.set_model")]
    SessionSetModel {
        id: String,
        session_id: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback_model: Option<Option<String>>,
    },
}

impl ClientMessage {
    /// Correlation id used to match a [`DaemonMessage::ResponseOk`](crate::daemon::DaemonMessage::ResponseOk)
    /// or [`DaemonMessage::ResponseError`](crate::daemon::DaemonMessage::ResponseError) back to this request.
    #[must_use]
    pub fn request_id(&self) -> &str {
        match self {
            Self::SessionCreate { id, .. }
            | Self::SessionList { id }
            | Self::SessionAttach { id, .. }
            | Self::SessionDetach { id, .. }
            | Self::SessionSend { id, .. }
            | Self::SessionInterrupt { id, .. }
            | Self::SessionApprove { id, .. }
            | Self::SessionDestroy { id, .. }
            | Self::SessionSetMode { id, .. }
            | Self::SessionPin { id, .. }
            | Self::SessionUnpin { id, .. }
            | Self::SessionRotate { id, .. }
            | Self::SessionSearch { id, .. }
            | Self::SessionSetModel { id, .. } => id,
        }
    }
}

/// One-shot attachment pushed with `session.send`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Base64-encoded bytes, mutually exclusive with `content`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// Mid-turn priority hint.
///
/// * `Now` — interrupt the agent's current turn and observe immediately.
/// * `Next` — let the current turn finish, then pick this up.
/// * `Later` — queue as a standard follow-up (default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendPriority {
    Now,
    Next,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    Workspace,
    All,
}
