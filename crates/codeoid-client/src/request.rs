//! In-flight request registry.
//!
//! Each outbound [`ClientMessage`](codeoid_protocol::ClientMessage) gets a
//! UUID id; the daemon echoes it on `response.ok` / `response.error` (or on
//! typed result messages like `session.list.result`). The registry maps
//! id → oneshot sender so the reader task can deliver the response without
//! the caller having to poll.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use codeoid_protocol::DaemonMessage;
use tokio::sync::oneshot;

use crate::error::{ClientError, Result};

/// The outcome a pending request awaits.
#[derive(Debug)]
pub enum RequestOutcome {
    /// Generic success — may carry a `data` payload.
    Ok(Option<serde_json::Value>),
    /// Typed result (for queries that return structured data, e.g. `session.list`).
    TypedResult(DaemonMessage),
    /// Daemon reported an error for this request.
    Error {
        code: codeoid_protocol::ErrorCode,
        message: String,
    },
}

/// Shared registry of in-flight requests. Cloneable — all clones point at
/// the same underlying map.
#[derive(Debug, Clone, Default)]
pub struct RequestRegistry {
    inner: Arc<Mutex<HashMap<String, oneshot::Sender<RequestOutcome>>>>,
}

impl RequestRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new in-flight request. Returns the receiver the caller
    /// awaits.
    pub fn register(&self, request_id: String) -> oneshot::Receiver<RequestOutcome> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .lock()
            .expect("registry mutex poisoned")
            .insert(request_id, tx);
        rx
    }

    /// Deliver a response. Returns `true` if we had a waiting sender for this
    /// request id, `false` if the request had already been cancelled or was
    /// never registered (in which case the outcome is dropped).
    pub fn complete(&self, request_id: &str, outcome: RequestOutcome) -> bool {
        let tx = self
            .inner
            .lock()
            .expect("registry mutex poisoned")
            .remove(request_id);
        match tx {
            Some(tx) => tx.send(outcome).is_ok(),
            None => false,
        }
    }

    /// Cancel a single pending request — used when the caller stops waiting
    /// (timeout) so a late response doesn't hit a dead oneshot.
    pub fn cancel(&self, request_id: &str) {
        self.inner
            .lock()
            .expect("registry mutex poisoned")
            .remove(request_id);
    }

    /// Cancel every pending request — used on shutdown and when the reader
    /// task exits (socket death), so awaiting callers unblock immediately
    /// instead of waiting out their timeout.
    pub fn cancel_all(&self) {
        let mut map = self.inner.lock().expect("registry mutex poisoned");
        map.clear();
    }
}

/// Convert a [`RequestOutcome`] into a successful payload or a typed
/// [`ClientError`]. Useful at call sites that want `?`-style error
/// propagation.
pub fn into_result(outcome: RequestOutcome, request_id: &str) -> Result<Option<serde_json::Value>> {
    match outcome {
        RequestOutcome::Ok(data) => Ok(data),
        RequestOutcome::TypedResult(_) => Ok(None),
        RequestOutcome::Error { code, message } => Err(ClientError::RequestFailed {
            request_id: request_id.to_string(),
            code,
            message,
        }),
    }
}
