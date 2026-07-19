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

use std::collections::HashMap;

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

    #[serde(rename = "models.list.result", rename_all = "camelCase")]
    ModelsListResult {
        request_id: String,
        models: Vec<ModelInfo>,
        /// True when these came from the live backend; false = built-in fallback.
        live: bool,
        /// Which backend this catalog is for — lets the client drop a stale
        /// result after a fast backend switch. Absent on older daemons.
        #[serde(default)]
        provider: Option<String>,
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
        /// `scrollback.paging`: this snapshot is only the NEWEST window;
        /// older history is fetched on demand via `scrollback.page`.
        #[serde(default)]
        tail: Option<bool>,
        /// With `tail: true` — whether history older than the window exists.
        #[serde(default)]
        has_more: Option<bool>,
    },

    /// Answer to `scrollback.page` — history strictly OLDER than the anchor,
    /// oldest→newest; the client PREPENDS (dedup by message id).
    #[serde(rename = "scrollback.page.result", rename_all = "camelCase")]
    ScrollbackPageResult {
        request_id: String,
        session_id: String,
        messages: Vec<SessionMessage>,
        has_more: bool,
        /// "buffer" | "transcript" — diagnostics only; kept as a string so
        /// new sources stay wire-additive.
        source: String,
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

    #[serde(rename = "session.export.result", rename_all = "camelCase")]
    SessionExportResult {
        request_id: String,
        manifest: SessionExportManifest,
        payload: SessionExportPayload,
    },

    #[serde(rename = "session.import.result", rename_all = "camelCase")]
    SessionImportResult {
        request_id: String,
        new_session_id: String,
        imported_messages: u32,
        imported_episodes: u32,
        imported_turns: u32,
        pinned_files_written: u32,
        warnings: Vec<String>,
    },

    /// Provider-initiated dialog (extension confirm gates, pick-one lists,
    /// free text). Only sent to clients that declared the `ui.dialogs`
    /// capability on their auth frame; answered with
    /// [`ClientMessage::SessionUiResponse`](crate::client::ClientMessage::SessionUiResponse).
    /// The daemon re-sends pending requests on attach and enforces
    /// `timeout_ms` itself — clients only display the countdown.
    #[serde(rename = "session.ui_request", rename_all = "camelCase")]
    SessionUiRequest(SessionUiRequestMsg),

    /// A dialog settled (answered here or elsewhere, timed out, or the turn
    /// was interrupted). Dismiss the local copy; unknown reasons = dismiss.
    #[serde(rename = "session.ui_resolved", rename_all = "camelCase")]
    SessionUiResolved {
        session_id: String,
        request_id: String,
        reason: UiResolvedReason,
        timestamp: String,
    },

    /// Reply to `session.commands` — the backing provider's slash-command
    /// catalog (extension commands, prompt templates, skills). Invoke by
    /// sending `"/name args"` as plain `session.send` text.
    #[serde(rename = "session.commands.result", rename_all = "camelCase")]
    SessionCommandsResult {
        request_id: String,
        session_id: String,
        provider_id: String,
        commands: Vec<ProviderCommand>,
    },

    /// Reply to `settings.schema` — the declarative settings manifest.
    #[serde(rename = "settings.schema.result", rename_all = "camelCase")]
    SettingsSchemaResult {
        request_id: String,
        manifest: SettingsManifest,
    },

    /// Reply to `settings.get` — current effective values + secret presence.
    #[serde(rename = "settings.get.result", rename_all = "camelCase")]
    SettingsGetResult {
        request_id: String,
        snapshot: SettingsSnapshot,
    },

    /// Reply to `settings.set` — outcome + the post-write snapshot.
    #[serde(rename = "settings.set.result", rename_all = "camelCase")]
    SettingsSetResult {
        request_id: String,
        ok: bool,
        snapshot: SettingsSnapshot,
        errors: Vec<SettingError>,
        restart_required: bool,
    },

    /// Forward-compat sink. Preserves raw JSON so the TUI can log it.
    #[serde(other)]
    Unknown,
}

// ── Settings manifest + snapshot (mirrors codeoid/packages/protocol settings.ts) ──

/// The declarative settings manifest served over `settings.schema`. Rendered
/// generically — `kind` / `backing` / `source` are kept as strings so a new
/// value the daemon introduces never breaks deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsManifest {
    pub version: u32,
    pub tabs: Vec<SettingsTab>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsTab {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub groups: Vec<SettingsGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsGroup {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub fields: Vec<SettingField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingField {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    /// "string" | "boolean" | "int" | "float" | "enum" | "string[]" | "secret".
    pub kind: String,
    /// "config" | "env".
    pub backing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<SettingOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub advanced: bool,
    #[serde(default)]
    pub secret: bool,
    /// "live" | "next-session" | "restart".
    #[serde(default)]
    pub applies: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingOption {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    /// key → current non-secret value + provenance.
    pub values: HashMap<String, SettingState>,
    /// key → secret presence + source (never the value).
    pub secrets: HashMap<String, SecretStatus>,
    pub config_path: String,
    pub env_path: String,
    /// Read-only registry MCP servers + live health (cross-backend mounter).
    /// Absent from older daemons — defaults to empty so deserialization is
    /// forward-compatible.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatus>,
}

/// Read-only status of one registry MCP server, mirrored from the TS protocol
/// (`McpServerStatus`). Config comes from the daemon's registry; `health`/`tools`
/// reflect what the daemon-owned client has observed so far (no live probe).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub name: String,
    /// "stdio" | "http" | "in-process".
    pub transport: String,
    /// "readonly" | "prompt".
    pub trust: String,
    /// "global" | "workspace" | "session".
    pub scope: String,
    /// Backends this server mounts on; `None` = all.
    #[serde(default)]
    pub backends: Option<Vec<String>>,
    pub enabled: bool,
    /// `codeoid_memory` — always present, not user-declared.
    pub builtin: bool,
    /// "connected" | "error" | "idle" | "disabled".
    pub health: String,
    pub tool_count: u32,
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingState {
    pub value: Value,
    /// "default" | "config" | "env" | "unset".
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub set: bool,
    /// "env-file" | "external" | "unset".
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingError {
    pub key: String,
    pub message: String,
}

/// Dialog flavor on a [`SessionUiRequestMsg`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRequestMethod {
    Select,
    Confirm,
    Input,
    Editor,
}

/// Why a `session.ui_resolved` fired. `Other` sinks future reasons — every
/// reason means "dismiss the local copy".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiResolvedReason {
    Answered,
    Cancelled,
    Timeout,
    Interrupted,
    #[serde(other)]
    Other,
}

/// Payload of [`DaemonMessage::SessionUiRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUiRequestMsg {
    pub session_id: String,
    /// Echo back on `session.ui_response`.
    pub request_id: String,
    pub method: UiRequestMethod,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Choices for `method: select`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefill: Option<String>,
    /// Auto-cancel deadline in ms from `timestamp` (daemon-enforced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    pub timestamp: String,
}

/// One provider-defined slash command (see `SessionCommandsResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCommand {
    /// Invokable name without the leading slash.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Provider-specific origin taxonomy (e.g. "extension" | "prompt" |
    /// "skill"). Display verbatim, never switch on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
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
    /// HTTP-type MCP servers' header keys (values redacted at the daemon).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_keys: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExportManifest {
    pub exported_at: String,
    pub session: SessionExportMetaSlim,
    pub workdir: SessionExportWorkdir,
    pub counts: SessionExportCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExportMetaSlim {
    pub id: String,
    pub name: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExportWorkdir {
    pub alias: String,
    pub alias_source: String,
    pub original_absolute: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExportCounts {
    pub messages: u32,
    pub episodes: u32,
    pub turns: u32,
    pub pinned_files: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionExportPayload {
    Inline {
        bundle: serde_json::Value,
        size_bytes: u64,
    },
    File {
        path: String,
        size_bytes: u64,
    },
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

/// One selectable model as reported by the Claude Code backend. Mirrors
/// `ModelInfo` in `codeoid/src/protocol/types.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// Value passed to `/model` and forwarded to the SDK (e.g. `"opus[1m]"`).
    pub value: String,
    /// Human label (e.g. `"Opus"`).
    pub display_name: String,
    /// Optional one-line description from the backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// True for the backend's recommended default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
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
    /// Capability identifiers the daemon supports (e.g. `commands.dynamic`,
    /// `ui.dialogs`). Feature-detect on these instead of version-sniffing.
    /// `None` on daemons that predate capability negotiation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    /// Provider ids registered on this daemon, default first (feeds the
    /// `/provider` command and the `/new --provider` flag). `None` on
    /// daemons that predate multi-provider session creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<String>>,
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
