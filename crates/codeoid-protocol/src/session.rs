//! Session metadata — the `SessionInfo` struct and its satellites.

use serde::{Deserialize, Serialize};

/// Lifecycle status of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Working,
    WaitingApproval,
    Error,
}

/// Execution mode — controls tool approval and autonomous budgeting.
///
/// * `Interactive` (default) — every tool call asks for approval.
/// * `AutoAllow` — Read/Grep/Glob/memory/recall are auto-approved; Write/Edit/Bash still ask.
/// * `Autonomous` — every tool auto-approved until the turn budget is exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionMode {
    Interactive,
    AutoAllow,
    Autonomous,
}

impl Default for SessionMode {
    fn default() -> Self {
        Self::Interactive
    }
}

/// Top-level session metadata — broadcast on attach, list, and every
/// `session.info_update`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub workdir: String,
    pub status: SessionStatus,
    pub created_by: String,
    pub created_at: String,
    pub attached_clients: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SessionMode>,

    /// Remaining turns budget for autonomous mode. `None` = unbounded, `Some(0)` = exhausted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns_remaining: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_files: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_uri: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<Subagent>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsage>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<RotationInfo>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_messages: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
}

/// Rotation telemetry — how many times the backing Claude Code session has
/// been rolled over to avoid context compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationInfo {
    pub count: u32,
    /// Unix ms of last rotation, or null if never rotated.
    pub last_rotated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code_session_id: Option<String>,
}

/// Cumulative usage totals for a session. Aggregated from each SDK `result` event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_cost_usd: f64,
    pub num_turns: u32,
    pub duration_ms: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_turns: Option<Vec<TurnUsage>>,

    /// Max PRIMARY-AGENT context size ever seen on a single turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_input_tokens: Option<u64>,

    /// Most recent turn's PRIMARY-AGENT context size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_input_tokens: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_output_tokens: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_cost_usd: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_cache_hit_rate: Option<f64>,
}

/// Per-turn usage record — one entry per SDK `result` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub turn_number: u32,
    pub created_at: i64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_cost_usd: f64,
    pub duration_ms: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,

    pub total_input_tokens: u64,
    pub billable_input_tokens: u64,
    pub cache_hit_rate: f64,
}

/// Active sub-agent for the identity chain display.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subagent {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wimse_uri: Option<String>,
    pub agent_type: String,
    pub spawned_at: i64,
    pub active: bool,
}
