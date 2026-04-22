//! Tool invocation state machine.
//!
//! Tool calls have a lifecycle: `streaming → waiting_confirmation → executing → completed`
//! (with `cancelled` as a terminal alternative). Each transition arrives as a
//! [`SessionMessageDelta`](crate::message::SessionMessageDelta) with a
//! `tool_state_update` field carrying the **whole next state**, not a patch.
//!
//! # Wire format
//!
//! Variant names serialize as `snake_case` to match the TS `ToolPhase` union
//! (`"streaming"` / `"waiting_confirmation"` / `"executing"` / `"completed"`
//! / `"cancelled"`). Inline variant fields serialize as `camelCase` to match
//! the TS interface shape (`partialInput`, `approvalId`, `elapsedMs`,
//! `confirmedBy`). The per-variant `rename_all = "camelCase"` attribute
//! handles the latter — a bare `rename_all` on the enum only affects
//! variant names, not their fields.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Phase name — useful as a lightweight discriminator when you don't need
/// the full state payload (e.g. to pick an icon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPhase {
    Streaming,
    WaitingConfirmation,
    Executing,
    Completed,
    Cancelled,
}

/// Tool state — a full replacement on each transition, never a delta.
///
/// Matching `toolStateUpdate` from the TS protocol, serialized as
/// `{"phase": "...", ...variant-specific-fields-in-camelCase}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ToolState {
    #[serde(rename_all = "camelCase")]
    Streaming {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partial_input: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    WaitingConfirmation {
        input: Value,
        description: String,
        approval_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Executing {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Completed {
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirmed_by: Option<ConfirmedBy>,
    },
    #[serde(rename_all = "camelCase")]
    Cancelled {
        reason: CancelReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

impl ToolState {
    #[must_use]
    pub fn phase(&self) -> ToolPhase {
        match self {
            Self::Streaming { .. } => ToolPhase::Streaming,
            Self::WaitingConfirmation { .. } => ToolPhase::WaitingConfirmation,
            Self::Executing { .. } => ToolPhase::Executing,
            Self::Completed { .. } => ToolPhase::Completed,
            Self::Cancelled { .. } => ToolPhase::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmedBy {
    User,
    Auto,
    Setting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    Denied,
    Interrupted,
    Timeout,
}

/// Metadata for an in-flight or finished tool invocation, attached to a
/// [`SessionMessage`](crate::message::SessionMessage) when `role = tool_call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub tool_id: String,
    pub name: String,
    pub state: ToolState,
}
