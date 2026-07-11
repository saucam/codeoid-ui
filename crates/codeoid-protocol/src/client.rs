//! Client → Daemon messages.
//!
//! Every request carries an `id` so the daemon's `response.ok` /
//! `response.error` / `session.list.result` can be correlated back to the
//! caller.
//!
//! # Wire-format notes
//!
//! The TS daemon expects `camelCase` fields (e.g. `sessionId`, `approvalId`).
//! Every struct-like variant below gets a per-variant `rename_all = "camelCase"`
//! so `session_id` serializes as `sessionId`, etc. `#[serde(rename_all)]` on
//! the enum itself only affects variant names, not variant fields — hence
//! the repetition.

use serde::{Deserialize, Serialize};
use serde_json;

use crate::session::SessionMode;

/// Tagged union of every message a client can send the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "session.create", rename_all = "camelCase")]
    SessionCreate {
        id: String,
        name: String,
        workdir: String,
        /// Backend for the session (one of `AuthOkMsg.providers`). The
        /// daemon fail-closes on unknown ids. Absent = daemon default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_id: Option<String>,
    },

    #[serde(rename = "session.list", rename_all = "camelCase")]
    SessionList { id: String },

    /// Request the backend's selectable model catalog. Daemon answers with
    /// [`DaemonMessage::ModelsListResult`](crate::daemon::DaemonMessage::ModelsListResult).
    #[serde(rename = "models.list", rename_all = "camelCase")]
    ModelsList { id: String },

    #[serde(rename = "session.attach", rename_all = "camelCase")]
    SessionAttach { id: String, session_id: String },

    #[serde(rename = "session.detach", rename_all = "camelCase")]
    SessionDetach { id: String, session_id: String },

    #[serde(rename = "session.send", rename_all = "camelCase")]
    SessionSend {
        id: String,
        session_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<Attachment>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        priority: Option<SendPriority>,
    },

    #[serde(rename = "session.interrupt", rename_all = "camelCase")]
    SessionInterrupt { id: String, session_id: String },

    #[serde(rename = "session.approve", rename_all = "camelCase")]
    SessionApprove {
        id: String,
        session_id: String,
        approval_id: String,
        approved: bool,
        /// Optional patch shallow-merged into the original tool input
        /// before the SDK runs the tool. Required for AskUserQuestion
        /// where it carries `{ "answers": { "<question>": "..." } }`.
        /// Omitted for binary approvals (Bash, Edit, ExitPlanMode, …).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
    },

    /// Answer a provider-initiated dialog
    /// ([`DaemonMessage::SessionUiRequest`](crate::daemon::DaemonMessage::SessionUiRequest)).
    /// Exactly one payload field applies per method: `value` for
    /// select/input/editor, `confirmed` for confirm, `cancelled: true` to
    /// dismiss any method. First answer wins; a late answer gets `not_found`
    /// (the `session.ui_resolved` broadcast already dismissed it).
    #[serde(rename = "session.ui_response", rename_all = "camelCase")]
    SessionUiResponse {
        id: String,
        session_id: String,
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirmed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cancelled: Option<bool>,
    },

    /// Activate a `ContentPart::Button` from a message's `parts[]`. The
    /// daemon validates the button exists on the real message before
    /// forwarding to the provider.
    #[serde(rename = "session.part_action", rename_all = "camelCase")]
    SessionPartAction {
        id: String,
        session_id: String,
        message_id: String,
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },

    /// Fetch the session's provider-defined command catalog. Daemon answers
    /// with [`DaemonMessage::SessionCommandsResult`](crate::daemon::DaemonMessage::SessionCommandsResult).
    /// Gated on the daemon capability `commands.dynamic`.
    #[serde(rename = "session.commands", rename_all = "camelCase")]
    SessionCommands { id: String, session_id: String },

    #[serde(rename = "session.destroy", rename_all = "camelCase")]
    SessionDestroy { id: String, session_id: String },

    #[serde(rename = "session.set_mode", rename_all = "camelCase")]
    SessionSetMode {
        id: String,
        session_id: String,
        mode: SessionMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_turns: Option<u32>,
    },

    #[serde(rename = "session.pin", rename_all = "camelCase")]
    SessionPin {
        id: String,
        session_id: String,
        path: String,
    },

    #[serde(rename = "session.unpin", rename_all = "camelCase")]
    SessionUnpin {
        id: String,
        session_id: String,
        path: String,
    },

    #[serde(rename = "session.rotate", rename_all = "camelCase")]
    SessionRotate { id: String, session_id: String },

    #[serde(rename = "session.search", rename_all = "camelCase")]
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

    /// Switch a live session's BACKEND (claude ⇄ pi …). The session id,
    /// scrollback, and transcript stay; the daemon carries the history to
    /// the new backend as a transcript and resets the model to its default.
    /// Rejected mid-turn and on unknown ids.
    #[serde(rename = "session.set_provider", rename_all = "camelCase")]
    SessionSetProvider {
        id: String,
        session_id: String,
        provider_id: String,
    },

    /// Branch a session into an independent one, optionally onto a different
    /// backend. The fork is seeded with a deep copy of the parent's canonical
    /// history + restamped scrollback; the parent is untouched. `name` absent
    /// ⇒ parent name + " (fork)"; `provider_id` absent ⇒ parent's backend.
    /// Fail-closed: foreign/unknown session ⇒ not_found, unknown provider ⇒
    /// invalid_request.
    #[serde(rename = "session.fork", rename_all = "camelCase")]
    SessionFork {
        id: String,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_id: Option<String>,
    },

    /// Fetch history OLDER than a message the client already holds
    /// (`scrollback.paging`). Anchored by message id — ids survive daemon
    /// restarts, seq cursors don't. Answered with `scrollback.page.result`.
    #[serde(rename = "scrollback.page", rename_all = "camelCase")]
    ScrollbackPage {
        id: String,
        session_id: String,
        /// The OLDEST message id the client currently holds.
        before_message_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },

    #[serde(rename = "session.set_model", rename_all = "camelCase")]
    SessionSetModel {
        id: String,
        session_id: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback_model: Option<Option<String>>,
    },

    #[serde(rename = "session.rename", rename_all = "camelCase")]
    SessionRename {
        id: String,
        session_id: String,
        name: String,
    },

    /// Snapshot of Claude Code config (`~/.claude/` + workdir `.claude/`)
    /// for the focused session — agents, skills, MCP servers, hooks.
    /// Daemon answers with `ClaudeConfigResult`.
    #[serde(rename = "claude.config", rename_all = "camelCase")]
    ClaudeConfig { id: String, session_id: String },

    /// Export a session as a portable `ShareBundle`.
    #[serde(rename = "session.export", rename_all = "camelCase")]
    SessionExport {
        id: String,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_memory: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_pinned_files: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias_override: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_file: Option<bool>,
    },

    /// Import a session bundle. The TUI typically uses the `file`
    /// variant (operator pastes a path); the web pushes inline JSON.
    #[serde(rename = "session.import", rename_all = "camelCase")]
    SessionImport {
        id: String,
        source: SessionImportSource,
        target_workdir: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name_override: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        write_pinned_files: Option<bool>,
    },
}

/// Source of a `session.import` bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionImportSource {
    Inline { bundle: serde_json::Value },
    File { path: String },
}

impl ClientMessage {
    /// Correlation id used to match a [`DaemonMessage::ResponseOk`](crate::daemon::DaemonMessage::ResponseOk)
    /// or [`DaemonMessage::ResponseError`](crate::daemon::DaemonMessage::ResponseError) back to this request.
    #[must_use]
    pub fn request_id(&self) -> &str {
        match self {
            Self::SessionCreate { id, .. }
            | Self::SessionList { id }
            | Self::ModelsList { id }
            | Self::SessionAttach { id, .. }
            | Self::SessionDetach { id, .. }
            | Self::SessionSend { id, .. }
            | Self::SessionInterrupt { id, .. }
            | Self::SessionApprove { id, .. }
            | Self::SessionUiResponse { id, .. }
            | Self::SessionPartAction { id, .. }
            | Self::SessionCommands { id, .. }
            | Self::SessionDestroy { id, .. }
            | Self::SessionSetMode { id, .. }
            | Self::SessionPin { id, .. }
            | Self::SessionUnpin { id, .. }
            | Self::SessionRotate { id, .. }
            | Self::SessionSearch { id, .. }
            | Self::SessionSetProvider { id, .. }
            | Self::SessionFork { id, .. }
            | Self::ScrollbackPage { id, .. }
            | Self::SessionSetModel { id, .. }
            | Self::SessionRename { id, .. }
            | Self::ClaudeConfig { id, .. }
            | Self::SessionExport { id, .. }
            | Self::SessionImport { id, .. } => id,
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
