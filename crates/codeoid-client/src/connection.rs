//! WebSocket connection lifecycle: connect → auth → spawn reader → hand out
//! a [`ClientHandle`] that the TUI uses to send requests and consume events.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use codeoid_protocol::{AuthOkMsg, ClientMessage, DaemonMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, trace, warn};
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
}

/// Cheap-to-clone handle the TUI keeps around to make requests.
#[derive(Clone)]
pub struct ClientHandle {
    tx: mpsc::Sender<Outbound>,
    registry: RequestRegistry,
    _reader: Arc<JoinHandle<()>>,
    _writer: Arc<JoinHandle<()>>,
    _heartbeat: Arc<JoinHandle<()>>,
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
    // Declare what THIS client can consume — the daemon only targets
    // capability-gated frames (session.ui_request) at connections that
    // declared them, so omitting `ui.dialogs` here would silently disable
    // provider dialogs for the TUI.
    let write = Arc::new(Mutex::new(write));
    {
        let mut w = write.lock().await;
        let auth_frame = serde_json::json!({
            "type": "auth",
            "token": token,
            "protocolVersion": codeoid_protocol::PROTOCOL_VERSION,
            "capabilities": ["parts", "ui.dialogs"],
            "client": format!("codeoid-tui/{}", env!("CARGO_PKG_VERSION")),
        });
        w.send(WsMessage::Text(auth_frame.to_string().into()))
            .await?;
    }

    // Step 2: wait for auth.ok as the next message. Bail on anything else.
    let mut read = read;
    let auth = await_auth_ok(&mut read).await?;

    // Step 3: hard version check. Greenfield project — daemon and TUI are
    // always deployed together. A mismatch is a misconfiguration, not a
    // user-facing warning.
    match auth.protocol_version {
        Some(v) if v == codeoid_protocol::PROTOCOL_VERSION => {}
        other => {
            return Err(ClientError::AuthRejected(format!(
                "protocol version mismatch: client speaks v{}, daemon speaks {:?}. \
                 Deploy daemon and TUI from the same commit.",
                codeoid_protocol::PROTOCOL_VERSION,
                other
            )));
        }
    }

    // Step 4: wire up the live stream.
    let registry = RequestRegistry::new();
    let (ev_tx, ev_rx) = mpsc::channel::<StreamEvent>(256);
    let (out_tx, out_rx) = mpsc::channel::<Outbound>(64);

    // Liveness: a shared monotonic "last frame received" timestamp the reader
    // bumps on every inbound frame and the heartbeat task reads to decide
    // when to ping / when to declare the socket dead.
    let base = Instant::now();
    let last_activity = Arc::new(AtomicU64::new(0));

    let reader_handle = spawn_reader(
        read,
        registry.clone(),
        ev_tx.clone(),
        last_activity.clone(),
        base,
    );
    let writer_handle = spawn_writer(write.clone(), out_rx);
    let heartbeat_handle = spawn_heartbeat(write.clone(), ev_tx, last_activity, base);

    Ok(Connected {
        handle: ClientHandle {
            tx: out_tx,
            registry,
            _reader: Arc::new(reader_handle),
            _writer: Arc::new(writer_handle),
            _heartbeat: Arc::new(heartbeat_handle),
        },
        events: ev_rx,
        auth,
    })
}

type ReadHalf = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
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
        Err(_) => Err(ClientError::AuthRejected(
            "timed out waiting for auth.ok".into(),
        )),
    }
}

fn spawn_reader(
    mut read: ReadHalf,
    registry: RequestRegistry,
    ev_tx: mpsc::Sender<StreamEvent>,
    last_activity: Arc<AtomicU64>,
    base: Instant,
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
            // Any inbound frame — text, ping, or the pong answering our
            // heartbeat — counts as liveness; reset the idle clock.
            last_activity.store(base.elapsed().as_millis() as u64, Ordering::Relaxed);
            match frame {
                WsMessage::Text(t) => {
                    trace!(bytes = t.len(), "ws recv text");
                    let msg: DaemonMessage = match serde_json::from_str(&t) {
                        Ok(m) => m,
                        Err(e) => {
                            // Dropping on parse failure is a protocol
                            // divergence — log the raw bytes so it's
                            // debuggable via CODEOID_LOG_FILE. Cap the
                            // preview so a huge scrollback replay doesn't
                            // blow up the log.
                            let preview: String = t.chars().take(800).collect();
                            warn!(
                                error = %e,
                                preview = %preview,
                                "daemon message failed to deserialize — DROPPED"
                            );
                            continue;
                        }
                    };
                    debug!(kind = daemon_kind(&msg), "daemon -> client");
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

// ── Heartbeat ──────────────────────────────────────────────────────────────
//
// Detect a dead socket and force a reconnect. We use WS-level Ping frames
// (the daemon's Bun.serve auto-answers with Pong) rather than the web
// client's app-level `{type:"ping"}` — browsers can't emit WS pings, so the
// web frontend simulates one; tungstenite can, so this is the cleaner
// primitive and needs no protocol round-trip. Tuned to match the web
// client's 20s-ping / 28s-dead window so all frontends behave alike.
const HEARTBEAT_CHECK_EVERY: Duration = Duration::from_secs(4);
const HEARTBEAT_PING_AFTER_MS: u64 = 20_000;
const HEARTBEAT_DEAD_AFTER_MS: u64 = 28_000;

fn spawn_heartbeat(
    write: WriteHalf,
    ev_tx: mpsc::Sender<StreamEvent>,
    last_activity: Arc<AtomicU64>,
    base: Instant,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(HEARTBEAT_CHECK_EVERY);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let now_ms = base.elapsed().as_millis() as u64;
            let idle_ms = now_ms.saturating_sub(last_activity.load(Ordering::Relaxed));

            if idle_ms >= HEARTBEAT_DEAD_AFTER_MS {
                warn!(
                    idle_ms,
                    "heartbeat: no traffic in liveness window — connection is dead"
                );
                let _ = ev_tx
                    .send(StreamEvent::Errored(ClientError::HeartbeatTimeout))
                    .await;
                return;
            }
            if idle_ms >= HEARTBEAT_PING_AFTER_MS {
                trace!(idle_ms, "heartbeat: sending ws ping");
                let mut w = write.lock().await;
                if w.send(WsMessage::Ping(Vec::new())).await.is_err() {
                    // The write half is gone (socket dead / replaced) — the
                    // reader will/has already surfaced the drop. Stop pinging.
                    let _ = ev_tx
                        .send(StreamEvent::Errored(ClientError::HeartbeatTimeout))
                        .await;
                    return;
                }
            }
        }
    })
}

fn spawn_writer(write: WriteHalf, mut rx: mpsc::Receiver<Outbound>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Outbound::Message(m) => {
                    let kind = client_kind(&m);
                    let payload = match serde_json::to_string(&m) {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(error = %e, kind, "failed to serialize outbound message");
                            continue;
                        }
                    };
                    debug!(kind, bytes = payload.len(), "client -> daemon");
                    trace!(kind, %payload, "client -> daemon wire");
                    let mut w = write.lock().await;
                    if let Err(e) = w.send(WsMessage::Text(payload.into())).await {
                        warn!(error = %e, kind, "writer error — closing");
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

async fn route(registry: &RequestRegistry, ev_tx: &mpsc::Sender<StreamEvent>, msg: DaemonMessage) {
    match msg {
        // Solicited — route to the request registry by id.
        DaemonMessage::ResponseOk { request_id, data } => {
            registry.complete(&request_id, RequestOutcome::Ok(data));
        }
        DaemonMessage::ResponseError {
            ref request_id,
            ref error,
            code,
        } => {
            let outcome = RequestOutcome::Error {
                code,
                message: error.clone(),
            };
            if !registry.complete(request_id, outcome) {
                // No one was awaiting this id (fire-and-forget send). Surface
                // the error to the app instead of dropping it.
                let _ = ev_tx.send(StreamEvent::Daemon(msg)).await;
            }
        }
        DaemonMessage::SessionListResult { .. }
        | DaemonMessage::ModelsListResult { .. }
        | DaemonMessage::SessionSearchResult { .. } => {
            let request_id = match &msg {
                DaemonMessage::SessionListResult { request_id, .. }
                | DaemonMessage::ModelsListResult { request_id, .. }
                | DaemonMessage::SessionSearchResult { request_id, .. } => request_id.clone(),
                _ => unreachable!(),
            };
            if !registry.complete(&request_id, RequestOutcome::TypedResult(clone_msg(&msg))) {
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

fn daemon_kind(msg: &DaemonMessage) -> &'static str {
    match msg {
        DaemonMessage::AuthOk(_) => "auth.ok",
        DaemonMessage::ResponseOk { .. } => "response.ok",
        DaemonMessage::ResponseError { .. } => "response.error",
        DaemonMessage::SessionListResult { .. } => "session.list.result",
        DaemonMessage::ModelsListResult { .. } => "models.list.result",
        DaemonMessage::SessionMessage(_) => "session.message",
        DaemonMessage::SessionMessageDelta(_) => "session.message.delta",
        DaemonMessage::SessionStatusChange { .. } => "session.status_change",
        DaemonMessage::SessionInfoUpdate { .. } => "session.info_update",
        DaemonMessage::ScrollbackReplay { .. } => "scrollback.replay",
        DaemonMessage::SessionSearchResult { .. } => "session.search.result",
        DaemonMessage::ClaudeConfigResult { .. } => "claude.config.result",
        DaemonMessage::SessionExportResult { .. } => "session.export.result",
        DaemonMessage::SessionImportResult { .. } => "session.import.result",
        DaemonMessage::SessionUiRequest(_) => "session.ui_request",
        DaemonMessage::SessionUiResolved { .. } => "session.ui_resolved",
        DaemonMessage::SessionCommandsResult { .. } => "session.commands.result",
        DaemonMessage::Unknown => "unknown",
    }
}

fn client_kind(msg: &ClientMessage) -> &'static str {
    match msg {
        ClientMessage::SessionCreate { .. } => "session.create",
        ClientMessage::SessionList { .. } => "session.list",
        ClientMessage::ModelsList { .. } => "models.list",
        ClientMessage::SessionAttach { .. } => "session.attach",
        ClientMessage::SessionDetach { .. } => "session.detach",
        ClientMessage::SessionSend { .. } => "session.send",
        ClientMessage::SessionInterrupt { .. } => "session.interrupt",
        ClientMessage::SessionApprove { .. } => "session.approve",
        ClientMessage::SessionUiResponse { .. } => "session.ui_response",
        ClientMessage::SessionPartAction { .. } => "session.part_action",
        ClientMessage::SessionCommands { .. } => "session.commands",
        ClientMessage::SessionDestroy { .. } => "session.destroy",
        ClientMessage::SessionSetMode { .. } => "session.set_mode",
        ClientMessage::SessionPin { .. } => "session.pin",
        ClientMessage::SessionUnpin { .. } => "session.unpin",
        ClientMessage::SessionRotate { .. } => "session.rotate",
        ClientMessage::SessionSearch { .. } => "session.search",
        ClientMessage::SessionSetModel { .. } => "session.set_model",
        ClientMessage::SessionRename { .. } => "session.rename",
        ClientMessage::ClaudeConfig { .. } => "claude.config",
        ClientMessage::SessionExport { .. } => "session.export",
        ClientMessage::SessionImport { .. } => "session.import",
    }
}

#[cfg(test)]
mod tests {
    use super::{client_kind, daemon_kind};
    use codeoid_protocol::{ClientMessage, DaemonMessage, SessionUiRequestMsg, UiRequestMethod};

    #[test]
    fn kind_maps_cover_the_provider_extension_surface() {
        let req = DaemonMessage::SessionUiRequest(SessionUiRequestMsg {
            session_id: "s".into(),
            request_id: "u".into(),
            method: UiRequestMethod::Confirm,
            title: "t".into(),
            message: None,
            options: None,
            placeholder: None,
            prefill: None,
            timeout_ms: None,
            timestamp: "t".into(),
        });
        assert_eq!(daemon_kind(&req), "session.ui_request");
        assert_eq!(
            daemon_kind(&DaemonMessage::SessionUiResolved {
                session_id: "s".into(),
                request_id: "u".into(),
                reason: codeoid_protocol::UiResolvedReason::Timeout,
                timestamp: "t".into(),
            }),
            "session.ui_resolved"
        );
        assert_eq!(
            daemon_kind(&DaemonMessage::SessionCommandsResult {
                request_id: "r".into(),
                session_id: "s".into(),
                provider_id: "pi".into(),
                commands: vec![],
            }),
            "session.commands.result"
        );

        assert_eq!(
            client_kind(&ClientMessage::SessionUiResponse {
                id: "1".into(),
                session_id: "s".into(),
                request_id: "u".into(),
                value: None,
                confirmed: Some(false),
                cancelled: None,
            }),
            "session.ui_response"
        );
        assert_eq!(
            client_kind(&ClientMessage::SessionPartAction {
                id: "1".into(),
                session_id: "s".into(),
                message_id: "m".into(),
                action: "a".into(),
                data: None,
            }),
            "session.part_action"
        );
        assert_eq!(
            client_kind(&ClientMessage::SessionCommands {
                id: "1".into(),
                session_id: "s".into(),
            }),
            "session.commands"
        );
    }
}
