//! Client error types.

use codeoid_protocol::ErrorCode;
use thiserror::Error;

pub type Result<T, E = ClientError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid daemon URL: {0}")]
    InvalidUrl(String),

    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("JSON encode/decode error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("daemon closed the connection before auth.ok")]
    HandshakeClosed,

    #[error("daemon rejected auth: {0}")]
    AuthRejected(String),

    #[error("daemon returned error on request {request_id}: [{code:?}] {message}")]
    RequestFailed {
        request_id: String,
        code: ErrorCode,
        message: String,
    },

    #[error("request {0} was cancelled before a response arrived")]
    RequestCancelled(String),

    #[error("daemon stream ended unexpectedly")]
    StreamClosed,

    #[error("internal channel send error")]
    ChannelClosed,

    #[error("heartbeat timed out — no traffic from daemon within the liveness window")]
    HeartbeatTimeout,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
