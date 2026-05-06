//! Daemon → Client messages.
//!
//! Everything the daemon can push over an attached WebSocket. Includes
//! solicited responses (correlated by `request_id`) and unsolicited events
//! (session messages, deltas, scrollback replay).
//!
//! # Wire format
//!
//! Per-variant `rename_all = "camelCase"` keeps field names in sync with
//! the TS `protocol/types.ts` shape without relying on per-field
//! `#[serde(rename = "…")]` hints. If you add a new variant, copy the
//! attribute — the `wire_no_snake_case` test will fail CI otherwise.
//!
//! # Forward compatibility
//!
//! The trailing `Unknown` variant is a sink for any `type` field the daemon
//! introduces that this crate doesn't know about. The TUI logs + ignores it,
//! matching the daemon's "frontends ignore unknown kinds" design.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::message::{MessageIdentity, SessionMessage, SessionMessageDelta};
use crate::session::{SessionInfo, SessionStatus};

/// Tagged union of every message the daemon can push to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonMessage {
    #[serde(rename = "auth.ok")]
    AuthOk(AuthOkMsg),

    #[serde(rename = "response.ok", rename_all = "camelCase")]
    ResponseOk {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },

    #[serde(rename = "response.error", rename_all = "camelCase")]
    ResponseError {
        request_id: String,
        error: String,
        code: ErrorCode,
    },

    #[serde(rename = "session.list.result", rename_all = "camelCase")]
    SessionListResult {
        request_id: String,
        sessions: Vec<SessionInfo>,
    },

    #[serde(rename = "session.message")]
    SessionMessage(SessionMessage),

    #[serde(rename = "session.message.delta")]
    SessionMessageDelta(SessionMessageDelta),

    #[serde(rename = "session.status_change", rename_all = "camelCase")]
    SessionStatusChange {
        session_id: String,
        status: SessionStatus,
        timestamp: String,
    },

    #[serde(rename = "session.info_update", rename_all = "camelCase")]
    SessionInfoUpdate {
        session: SessionInfo,
        timestamp: String,
    },

    #[serde(rename = "scrollback.replay", rename_all = "camelCase")]
    ScrollbackReplay {
        session_id: String,
        messages: Vec<SessionMessage>,
    },

    #[serde(rename = "session.search.result", rename_all = "camelCase")]
    SessionSearchResult {
        request_id: String,
        query: String,
        sessions: Vec<SessionSearchHit>,
        workspace_id: String,
        limit: u32,
    },

    #[serde(rename = "claude.config.result", rename_all = "camelCase")]
    ClaudeConfigResult {
        request_id: String,
        workdir: String,
        agents: Vec<ClaudeConfigAgent>,
        skills: Vec<ClaudeConfigSkill>,
        mcp_servers: Vec<ClaudeConfigMcpServer>,
        hooks: Vec<ClaudeConfigHook>,
    },

    /// Forward-compat sink. Preserves raw JSON so the TUI can log it.
    #[serde(other)]
    Unknown,
}

/// Where the config entry was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeConfigScope {
    Global,
    Workdir,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeConfigAgent {
    pub name: String,
    pub description: Option<String>,
    pub path: String,
    pub scope: ClaudeConfigScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeConfigSkill {
    pub name: String,
    pub description: Option<String>,
    pub path: String,
    pub scope: ClaudeConfigScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeConfigMcpServer {
    pub name: String,
    pub scope: ClaudeConfigScope,
    pub path: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub url: Option<String>,
    #[serde(rename = "type")]
    pub server_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeConfigHook {
    pub event: String,
    pub scope: ClaudeConfigScope,
    pub path: String,
    pub matcher: Option<String>,
    pub kind: String,
    pub command: String,
}

/// Sent after a successful auth handshake, before any other traffic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthOkMsg {
    pub identity: MessageIdentity,
    pub scopes: Vec<String>,
    /// Wire-protocol version the daemon speaks. Compare against
    /// [`crate::PROTOCOL_VERSION`]. `None` means a pre-v1 daemon that didn't
    /// send the field — treat as version 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    Forbidden,
    NotFound,
    InvalidRequest,
    RateLimited,
    Internal,
}

/// Per-session hit returned by `session.search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchHit {
    pub session_id: String,
    pub session_name: String,
    pub workdir: String,
    pub match_count: u32,
    pub first_match_at: i64,
    pub last_match_at: i64,
    pub aggregate_score: f64,
    pub snippets: Vec<SessionSearchSnippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchSnippet {
    pub episode_id: String,
    pub kind: SearchSnippetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub summary: String,
    pub excerpt: String,
    pub created_at: i64,
    pub score: f64,
    pub file_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSnippetKind {
    UserTurn,
    AssistantTurn,
    ToolCall,
    Error,
}
