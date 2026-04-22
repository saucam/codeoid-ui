//! WebSocket connection lifecycle: connect → auth → spawn reader → hand out
//! a [`ClientHandle`] that the TUI uses to send requests and consume events.

use std::sync::Arc;
use std::time::Duration;

use codeoid_protocol::{AuthOkMsg, ClientMessage, DaemonMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};
use url::Url;
use uuid::Uuid;

use crate::error::{ClientError, Result};
use crate::request::{into_result, RequestOutcome, RequestRegistry};

/// Unsolicited events that flow from the daemon into the TUI event loop.
///
/// This intentionally excludes [`DaemonMessage::AuthOk`] (consumed during
/// handshake) and [`DaemonMessage::ResponseOk`] / `ResponseError` /
/// `SessionListResult` (routed to the request registry). Everything else
/// reaches the TUI untouched.
#[derive(Debug)]
pub enum StreamEvent {
    Daemon(DaemonMessage),
    /// Connection closed cleanly by the peer.
    Closed,
    /// Connection died with an error — TUI should surface + offer reconnect.
    Errored(ClientError),
}

/// Successful connect outcome.
#[derive(Debug)]
pub struct Connected {
    pub handle: ClientHandle,
    pub events: mpsc::Receiver<StreamEvent>,
    pub auth: AuthOkMsg,
    /// Version drift warning if the daemon's `PROTOCOL_VERSION` does not match
    /// [`codeoid_protocol::PROTOCOL_VERSION`]. `None` means aligned.
    pub version_warning: Option<VersionDrift>,
}

#[derive(Debug, Clone, Copy)]
pub struct VersionDrift {
    pub client: u32,
    pub daemon: Option<u32>,
}

/// Cheap-to-clone handle the TUI keeps around to make requests.
#[derive(Clone)]
pub struct ClientHandle {
    tx: mpsc::Sender<Outbound>,
    registry: RequestRegistry,
    _reader: Arc<JoinHandle<()>>,
    _writer: Arc<JoinHandle<()>>,
}

impl std::fmt::Debug for ClientHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientHandle").finish_non_exhaustive()
    }
}

enum Outbound {
    Message(ClientMessage),
    Shutdown,
}

impl ClientHandle {
    /// Fire-and-forget send of a [`ClientMessage`] — no response awaited.
    /// Use this for notifications like `session.interrupt` where the daemon
    /// may not acknowledge.
    pub async fn send(&self, msg: ClientMessage) -> Result<()> {
        self.tx
            .send(Outbound::Message(msg))
            .await
            .map_err(|_| ClientError::ChannelClosed)
    }

    /// Send a [`ClientMessage`] and await its correlated response.
    ///
    /// Note: the caller must construct `msg` with a unique id. Use
    /// [`Self::next_request_id`] if you don't have one handy.
    pub async fn request(&self, msg: ClientMessage) -> Result<RequestOutcome> {
        let id = msg.request_id().to_string();
        let rx = self.registry.register(id.clone());
        self.tx
            .send(Outbound::Message(msg))
            .await
            .map_err(|_| ClientError::ChannelClosed)?;
        rx.await
            .map_err(|_| ClientError::RequestCancelled(id.clone()))
    }

    /// Convenience: request and unwrap to a JSON payload, bubbling typed
    /// daemon errors via [`ClientError::RequestFailed`].
    pub async fn request_ok(&self, msg: ClientMessage) -> Result<Option<serde_json::Value>> {
        let id = msg.request_id().to_string();
        let outcome = self.request(msg).await?;
        into_result(outcome, &id)
    }

    /// Close the connection gracefully.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(Outbound::Shutdown).await;
        self.registry.cancel_all();
    }

    /// Generate a fresh request id.
    #[must_use]
    pub fn next_request_id() -> String {
        Uuid::new_v4().to_string()
    }
}

/// Connect to a daemon, authenticate, and return a ready-to-use handle.
///
/// `url` must be a `ws://` or `wss://` URL (no trailing path is required;
/// the daemon serves the socket at `/`).
///
/// `token` is the ZeroID JWT that the daemon's [`verifyToken`] accepts. For
/// local dev, use the token printed by `codeoid auth`.
pub async fn connect(url: &str, token: &str) -> Result<Connected> {
    let mut parsed = Url::parse(url).map_err(|e| ClientError::InvalidUrl(e.to_string()))?;

    // Auth goes as a query param — the daemon's handshake code reads it off
    // the first message, but several deployments also accept it as `?token=`.
    // We use the first-message path here (mirrors the TS terminal client).
    // Strip any user-provided token from the URL so we don't double-send.
    parsed.set_query(None);

    debug!(url = %parsed, "connecting to daemon");

    let (ws, _resp) = tokio_tungstenite::connect_async(parsed.as_str()).await?;
    let (write, read) = ws.split();

    // Step 1: send auth token as the first WS frame (TS client pattern).
    let write = Arc::new(Mutex::new(write));
    {
        let mut w = write.lock().await;
        let auth_frame = serde_json::json!({ "type": "auth", "token": token });
        w.send(WsMessage::Text(auth_frame.to_string().into())).await?;
    }

    // Step 2: wait for auth.ok as the next message. Bail on anything else.
    let mut read = read;
    let auth = await_auth_ok(&mut read).await?;

    // Step 3: compute version drift.
    let version_warning = match auth.protocol_version {
        Some(v) if v == codeoid_protocol::PROTOCOL_VERSION => None,
        other => Some(VersionDrift {
            client: codeoid_protocol::PROTOCOL_VERSION,
            daemon: other,
        }),
    };
    if let Some(drift) = version_warning {
        warn!(
            client_version = drift.client,
            daemon_version = ?drift.daemon,
            "protocol version drift detected"
        );
    }

    // Step 4: wire up the live stream.
    let registry = RequestRegistry::new();
    let (ev_tx, ev_rx) = mpsc::channel::<StreamEvent>(256);
    let (out_tx, out_rx) = mpsc::channel::<Outbound>(64);

    let reader_handle = spawn_reader(read, registry.clone(), ev_tx.clone());
    let writer_handle = spawn_writer(write.clone(), out_rx);

    Ok(Connected {
        handle: ClientHandle {
            tx: out_tx,
            registry,
            _reader: Arc::new(reader_handle),
            _writer: Arc::new(writer_handle),
        },
        events: ev_rx,
        auth,
        version_warning,
    })
}

type ReadHalf =
    futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
type WriteHalf = Arc<
    Mutex<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>>,
>;

async fn await_auth_ok(read: &mut ReadHalf) -> Result<AuthOkMsg> {
    // Give the daemon 10 seconds to respond — longer than any plausible
    // network hop, shorter than a hung TCP retransmit.
    let deadline = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let Some(frame) = read.next().await else {
                return Err(ClientError::HandshakeClosed);
            };
            let frame = frame?;
            match frame {
                WsMessage::Text(t) => {
                    let msg: DaemonMessage = serde_json::from_str(&t)?;
                    match msg {
                        DaemonMessage::AuthOk(ok) => return Ok(ok),
                        DaemonMessage::ResponseError { error, .. } => {
                            return Err(ClientError::AuthRejected(error))
                        }
                        // The daemon may emit ping frames or other chatter
                        // before auth is complete in certain dev modes —
                        // log + ignore.
                        other => {
                            debug!(?other, "ignoring pre-auth message");
                        }
                    }
                }
                WsMessage::Close(frame) => {
                    let reason = frame
                        .map(|f| format!("{}: {}", f.code, f.reason))
                        .unwrap_or_else(|| "no reason".to_string());
                    return Err(ClientError::AuthRejected(reason));
                }
                WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Binary(_) => {}
                WsMessage::Frame(_) => {}
            }
        }
    });

    match deadline.await {
        Ok(result) => result,
        Err(_) => Err(ClientError::AuthRejected("timed out waiting for auth.ok".into())),
    }
}

fn spawn_reader(
    mut read: ReadHalf,
    registry: RequestRegistry,
    ev_tx: mpsc::Sender<StreamEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(frame) = read.next().await {
            let frame = match frame {
                Ok(f) => f,
                Err(e) => {
                    let _ = ev_tx
                        .send(StreamEvent::Errored(ClientError::WebSocket(e)))
                        .await;
                    return;
                }
            };
            match frame {
                WsMessage::Text(t) => {
                    let msg: DaemonMessage = match serde_json::from_str(&t) {
                        Ok(m) => m,
                        Err(e) => {
                            warn!(error = %e, "failed to parse daemon message; dropping");
                            continue;
                        }
                    };
                    route(&registry, &ev_tx, msg).await;
                }
                WsMessage::Close(_) => {
                    let _ = ev_tx.send(StreamEvent::Closed).await;
                    return;
                }
                WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Binary(_) => {}
                WsMessage::Frame(_) => {}
            }
        }
        let _ = ev_tx.send(StreamEvent::Closed).await;
    })
}

fn spawn_writer(write: WriteHalf, mut rx: mpsc::Receiver<Outbound>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Outbound::Message(m) => {
                    let payload = match serde_json::to_string(&m) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(error = %e, "failed to serialize outbound message");
                            continue;
                        }
                    };
                    let mut w = write.lock().await;
                    if let Err(e) = w.send(WsMessage::Text(payload.into())).await {
                        warn!(error = %e, "writer error — closing");
                        return;
                    }
                }
                Outbound::Shutdown => {
                    let mut w = write.lock().await;
                    let _ = w.send(WsMessage::Close(None)).await;
                    return;
                }
            }
        }
    })
}

async fn route(
    registry: &RequestRegistry,
    ev_tx: &mpsc::Sender<StreamEvent>,
    msg: DaemonMessage,
) {
    match msg {
        // Solicited — route to the request registry by id.
        DaemonMessage::ResponseOk { request_id, data } => {
            registry.complete(&request_id, RequestOutcome::Ok(data));
        }
        DaemonMessage::ResponseError {
            request_id,
            error,
            code,
        } => {
            registry.complete(
                &request_id,
                RequestOutcome::Error {
                    code,
                    message: error,
                },
            );
        }
        DaemonMessage::SessionListResult { .. }
        | DaemonMessage::SessionSearchResult { .. } => {
            let request_id = match &msg {
                DaemonMessage::SessionListResult { request_id, .. }
                | DaemonMessage::SessionSearchResult { request_id, .. } => request_id.clone(),
                _ => unreachable!(),
            };
            if !registry.complete(
                &request_id,
                RequestOutcome::TypedResult(clone_msg(&msg)),
            ) {
                // No one was waiting — forward as an event so the TUI can
                // still display it.
                let _ = ev_tx.send(StreamEvent::Daemon(msg)).await;
            }
        }
        // Unsolicited — forward to the TUI.
        other => {
            if let DaemonMessage::Unknown = other {
                warn!("received unknown daemon message kind — ignoring");
                return;
            }
            if ev_tx.send(StreamEvent::Daemon(other)).await.is_err() {
                // Receiver dropped — app is shutting down.
            }
        }
    }
}

// DaemonMessage is Clone; this helper keeps the match arms readable when we
// need both "forward" and "correlate" paths.
fn clone_msg(msg: &DaemonMessage) -> DaemonMessage {
    msg.clone()
}
