//! Async client for the Codeoid daemon.
//!
//! # Shape
//!
//! * [`connect`] opens a WebSocket, sends the auth token, waits for `auth.ok`,
//!   spawns a reader task, and hands back a [`Client`] handle plus a stream of
//!   unsolicited [`DaemonMessage`]s.
//! * [`Client::request`] correlates a request id to a oneshot channel, sends
//!   the message, and awaits either `response.ok`, `response.error`, or a
//!   typed result message (e.g. `session.list.result`).
//! * Unsolicited traffic — session messages, deltas, status changes,
//!   scrollback replay — is routed to a [`tokio::sync::mpsc`] receiver that
//!   the TUI owns.
//!
//! The TUI never touches WebSocket bytes. It only sees Rust types.

#![deny(missing_debug_implementations)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod auth;
pub mod config;
pub mod connection;
pub mod error;
pub mod request;

pub use auth::{resolve_token, AuthError};
pub use config::{config_dir, load_file_config, resolve_zeroid_url, FileConfig};
pub use connection::{connect, ClientHandle, Connected, StreamEvent};
pub use error::{ClientError, Result};
pub use request::{into_result, RequestOutcome, RequestRegistry};
