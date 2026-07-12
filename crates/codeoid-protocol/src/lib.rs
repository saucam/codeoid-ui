//! Codeoid wire protocol — Rust port of `src/protocol/types.ts` from the daemon.
//!
//! This crate is pure data: no I/O, no async, no Tokio. It defines serde-
//! compatible types for every message that flows between a Codeoid client and
//! the daemon's WebSocket server.
//!
//! # Design notes
//!
//! * **Discriminated unions.** TypeScript uses `type: "foo"` and `kind: "bar"`
//!   as discriminants. In Rust these map to `#[serde(tag = "type")]` and
//!   `#[serde(tag = "kind")]` tagged enums.
//! * **Forward compatibility.** The daemon's design explicitly says "frontends
//!   ignore unknown kinds". We mirror that by using `#[serde(other)]` on a
//!   trailing `Unknown` variant and `#[serde(flatten)]` on extensible metadata
//!   maps. Adding new server-side message kinds never breaks an old Rust
//!   client — they deserialize into [`DaemonMessage::Unknown`] and the TUI
//!   logs + skips them.
//! * **Tool state as full replacement.** [`ToolState`] is a tagged enum where
//!   each phase carries its own shape. When a [`SessionMessageDelta`] arrives
//!   with `tool_state_update`, replace the whole [`ToolInfo::state`] — do not
//!   merge fields.
//!
//! # Versioning
//!
//! [`PROTOCOL_VERSION`] is compared against the `protocol_version` field the
//! daemon sends on `auth.ok`. Mismatch is non-fatal (the daemon's additive
//! changes keep old clients working), but the client warns the user so they
//! can upgrade.

#![deny(missing_debug_implementations)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod client;
pub mod daemon;
pub mod message;
pub mod session;
pub mod tool;

pub use client::{Attachment, ClientMessage, SearchScope, SendPriority, SessionImportSource};
pub use daemon::{
    AuthOkMsg, ClaudeConfigAgent, ClaudeConfigHook, ClaudeConfigMcpServer, ClaudeConfigScope,
    ClaudeConfigSkill, DaemonMessage, ErrorCode, ModelInfo, ProviderCommand, SessionExportCounts,
    SessionExportManifest, SessionExportMetaSlim, SessionExportPayload, SessionExportWorkdir,
    SessionSearchHit, SessionSearchSnippet, SessionUiRequestMsg, UiRequestMethod, UiResolvedReason,
};
pub use message::{
    ContentPart, IdentityType, MessageIdentity, MessageRole, SessionMessage, SessionMessageDelta,
};
pub use session::{
    ForkedFrom, SessionInfo, SessionMode, SessionStatus, SessionUsage, Subagent, TurnUsage,
};
pub use tool::{CancelReason, ConfirmedBy, ToolInfo, ToolPhase, ToolState};

/// Wire protocol version this crate speaks.
///
/// Compared against [`AuthOkMsg::protocol_version`] on connect. Bump this
/// alongside the daemon's `PROTOCOL_VERSION` on any breaking change.
pub const PROTOCOL_VERSION: u32 = 1;
