//! The app reducer — owns `AppState`, drains the merged event stream, and
//! hands the renderer a snapshot on each tick.
//!
//! Connection lifecycle is modelled explicitly: the outer loop handles
//! connect + reconnect-with-backoff, the inner loop handles events for the
//! currently-live connection. Dropping a connection transitions
//! `AppState::connection` through `Reconnecting { attempt, next_attempt_in_secs }`
//! so the status bar pills update visibly.

use std::io;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use codeoid_client::{connect, ClientHandle, Connected, StreamEvent};
use codeoid_protocol::{
    ClientMessage, DaemonMessage, SessionMode, SessionStatus, ToolState,
};
use crossterm::event::{Event as CtEvent, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, Instant, Interval};
use tracing::{debug, info, warn};

use crate::commands::{self, SlashCommand};
use crate::event::AppEvent;
use crate::keymap::{resolve, Action};
use crate::state::{AppState, ConnectionState, Focus, Modal};
use crate::ui;

const TICK: Duration = Duration::from_millis(100);

/// Reconnect budget. Beyond this we give up and surface a terminal error.
const MAX_RECONNECT_ATTEMPTS: u32 = 5;

pub struct App {
    url: String,
    token: String,
    state: Option<AppState>,
    handle: Option<ClientHandle>,
    daemon_events: Option<mpsc::Receiver<StreamEvent>>,
    quit_requested: bool,
}

impl App {
    #[must_use]
    pub fn new(url: String, token: String) -> Self {
        Self {
            url,
            token,
            state: None,
            handle: None,
            daemon_events: None,
            quit_requested: false,
        }
    }

    pub async fn run(
        mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let mut term_events = EventStream::new();
        let mut ticker = interval(TICK);

        // Initial connect. If this fails we propagate so the user sees the
        // real diagnostic (wrong token, wrong url) rather than landing in
        // an empty TUI.
        let connected = connect(&self.url, &self.token)
            .await
            .context("initial connect to daemon")?;
        self.absorb_connection(connected);
        self.on_connected().await;

        'outer: loop {
            // Run the event loop for the currently-live connection. It
            // returns when we quit or when the connection drops.
            let disconnected = self
                .event_loop(terminal, &mut term_events, &mut ticker)
                .await?;

            if self.quit_requested {
                break 'outer;
            }

            if !disconnected {
                // Loop exited without a quit and without a disconnect —
                // shouldn't happen, but be defensive.
                break 'outer;
            }

            // Connection is gone. Attempt a bounded reconnect.
            if let Err(fatal) = self.reconnect_with_backoff(terminal).await {
                if let Some(state) = self.state.as_mut() {
                    state.connection = ConnectionState::Failed {
                        reason: fatal.to_string(),
                    };
                    state.record_error(format!("disconnected: {fatal}"));
                    // One final draw so the user can read the failure.
                    let state_mut: &mut AppState = state;
                    terminal.draw(|f| ui::render(f, state_mut))?;
                }
                // Brief pause so the failure pill is legible.
                sleep(Duration::from_secs(2)).await;
                break 'outer;
            }
        }

        if let Some(handle) = self.handle.as_ref() {
            handle.shutdown().await;
        }
        Ok(())
    }

    /// Inner loop. Returns `Ok(true)` if the connection dropped (caller
    /// should reconnect) and `Ok(false)` if we exited cleanly (quit).
    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        term_events: &mut EventStream,
        ticker: &mut Interval,
    ) -> Result<bool> {
        // First frame — always paint.
        let mut dirty = true;

        loop {
            if dirty {
                if let Some(state) = self.state.as_mut() {
                    terminal.draw(|f| ui::render(f, state))?;
                }
                dirty = false;
            }

            let events = self
                .daemon_events
                .as_mut()
                .ok_or_else(|| anyhow!("no daemon events channel"))?;

            let event = tokio::select! {
                Some(ev) = term_events.next() => match ev {
                    Ok(e) => AppEvent::Terminal(e),
                    Err(e) => {
                        warn!(error = %e, "terminal event stream error");
                        continue;
                    }
                },
                Some(ev) = events.recv() => AppEvent::Net(ev),
                _ = ticker.tick() => AppEvent::Tick,
            };

            let is_tick = matches!(event, AppEvent::Tick);

            match &event {
                AppEvent::Net(StreamEvent::Closed | StreamEvent::Errored(_)) => {
                    // Surface the reason, then return up so reconnect can
                    // take over.
                    if let Some(state) = self.state.as_mut() {
                        match &event {
                            AppEvent::Net(StreamEvent::Closed) => {
                                state.record_error("daemon closed the connection");
                            }
                            AppEvent::Net(StreamEvent::Errored(err)) => {
                                state.record_error(format!("daemon error: {err}"));
                            }
                            _ => {}
                        }
                    }
                    return Ok(true);
                }
                _ => {}
            }

            self.update(event).await;

            if self.quit_requested {
                return Ok(false);
            }

            // Redraw policy: anything user-initiated or daemon-driven
            // marks the UI dirty. Ticks only redraw if an animation is
            // actually running — otherwise we'd repaint the screen 10×/s
            // for no reason, destroying native terminal text selection.
            dirty = if is_tick {
                self.state
                    .as_ref()
                    .is_some_and(needs_animation_frame)
            } else {
                true
            };
        }
    }

    async fn update(&mut self, event: AppEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            AppEvent::Terminal(CtEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                let prompt_focused = state.focus == Focus::Prompt;
                let modal_open = state.modal.is_some();
                let command_mode = state.is_command_mode();
                if let Some(action) = resolve(key, prompt_focused, modal_open, command_mode) {
                    self.apply_action(action).await;
                } else if prompt_focused && !modal_open {
                    // Anything keymap didn't handle goes straight to the
                    // TextArea — arrows, Home/End, Ctrl+A/E, word-delete,
                    // selection, paste, the lot.
                    state.prompt.input(key);
                }
            }
            AppEvent::Terminal(_) => {}
            AppEvent::Net(StreamEvent::Daemon(msg)) => self.apply_daemon(msg),
            AppEvent::Net(_) => {
                // Closed/Errored already handled in event_loop.
            }
            AppEvent::Tick => state.tick(),
        }
    }

    async fn apply_action(&mut self, action: Action) {
        let Some(state) = self.state.as_mut() else { return };
        match action {
            Action::Quit => self.quit_requested = true,
            Action::FocusPrompt => state.focus = Focus::Prompt,
            Action::BlurPrompt => state.focus = Focus::Scrollback,
            Action::SubmitPrompt => self.submit_prompt().await,
            Action::NewlineInPrompt => {
                state.prompt.insert_newline();
            }
            Action::AutocompleteCommand => {
                autocomplete_command(state);
            }
            Action::NextSession => {
                state.sessions.focus_next();
                state.scroll_to_bottom();
                self.ensure_attached().await;
            }
            Action::PrevSession => {
                state.sessions.focus_prev();
                state.scroll_to_bottom();
                self.ensure_attached().await;
            }
            Action::Interrupt => self.interrupt().await,
            Action::Approve => self.approve(true).await,
            Action::Deny => self.approve(false).await,
            Action::CycleMode => {
                info!("cycle mode: not yet implemented");
            }
            Action::ToggleHelp => {
                state.modal = match state.modal {
                    Some(Modal::Help) => None,
                    _ => Some(Modal::Help),
                };
            }
            Action::DismissModal => {
                state.modal = None;
            }
            Action::ScrollUp => state.scroll_up(1),
            Action::ScrollDown => state.scroll_down(1),
            Action::PageUp => state.scroll_up(page_step(state)),
            Action::PageDown => state.scroll_down(page_step(state)),
            Action::ScrollToTop => state.scroll_to_top(),
            Action::ScrollToBottom => state.scroll_to_bottom(),
        }
    }

    fn apply_daemon(&mut self, msg: DaemonMessage) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match msg {
            DaemonMessage::SessionListResult { sessions, .. } => {
                state.set_sessions(sessions);
                let to_attach = state.sessions.focused_id().map(ToString::to_string);
                if let Some(id) = to_attach {
                    // Queue a defensive attach for the freshly-focused
                    // session. We don't await here — apply_daemon runs
                    // inside update() which holds a &mut self reborrow,
                    // and awaiting would require more restructuring. The
                    // user's first send will await-attach before sending,
                    // which is what actually matters for correctness.
                    let _ = self
                        .state
                        .as_mut()
                        .map(|s| s.note_attached(&id))
                        .unwrap_or(false);
                    if let Some(handle) = self.handle.clone() {
                        tokio::spawn(async move {
                            let msg = ClientMessage::SessionAttach {
                                id: ClientHandle::next_request_id(),
                                session_id: id,
                            };
                            let _ = handle.send(msg).await;
                        });
                    }
                }
            }
            DaemonMessage::SessionInfoUpdate { session, .. } => {
                state.merge_session(session);
            }
            DaemonMessage::SessionStatusChange {
                session_id, status, ..
            } => {
                if let Some(s) = state
                    .sessions
                    .items()
                    .iter()
                    .find(|s| s.id == session_id)
                    .cloned()
                {
                    let mut updated = s;
                    updated.status = status;
                    state.merge_session(updated);
                }
            }
            DaemonMessage::SessionMessage(m) => {
                let produces_text = matches!(
                    m.role,
                    codeoid_protocol::MessageRole::Assistant
                        | codeoid_protocol::MessageRole::Thinking
                );
                let session_id = m.session_id.clone();
                state.messages.apply_message(m);
                if produces_text {
                    state.mark_activity(&session_id);
                }
            }
            DaemonMessage::SessionMessageDelta(d) => {
                let session_id = d.session_id.clone();
                state.messages.apply_delta(d);
                state.mark_activity(&session_id);
            }
            DaemonMessage::ScrollbackReplay {
                session_id,
                messages,
            } => state.messages.replace_scrollback(session_id, messages),
            DaemonMessage::ResponseError { error, code, .. } => {
                state.record_error(format!("daemon error [{code:?}]: {error}"));
            }
            DaemonMessage::SessionSearchResult { .. }
            | DaemonMessage::AuthOk(_)
            | DaemonMessage::ResponseOk { .. } => {
                // Solicited; handled by the request registry.
            }
            DaemonMessage::Unknown => {
                warn!("received unknown daemon message; forward-compat drop");
            }
        }
    }

    // -------- connection lifecycle --------

    fn absorb_connection(&mut self, connected: Connected) {
        let state = self
            .state
            .get_or_insert_with(|| AppState::new(connected.auth.clone()));
        state.connection = ConnectionState::Connected;
        // Always clear attach ledger on new connection — the daemon has
        // zero state about us.
        state.attached.clear();
        self.handle = Some(connected.handle);
        self.daemon_events = Some(connected.events);
    }

    async fn on_connected(&mut self) {
        // Kick off an initial session list so the tabs aren't empty.
        if let Some(handle) = self.handle.as_ref() {
            let id = ClientHandle::next_request_id();
            if let Err(e) = handle.send(ClientMessage::SessionList { id }).await {
                warn!(error = %e, "failed to send initial session.list");
            }
        }
    }

    async fn reconnect_with_backoff(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        for attempt in 1..=MAX_RECONNECT_ATTEMPTS {
            let delay_secs = (1u64 << (attempt - 1)).min(30);
            if let Some(state) = self.state.as_mut() {
                state.connection = ConnectionState::Reconnecting {
                    attempt,
                    next_attempt_in_secs: delay_secs,
                };
                terminal.draw(|f| ui::render(f, state))?;
            }

            debug!(attempt, delay_secs, "reconnect attempt pending");
            let deadline = Instant::now() + Duration::from_secs(delay_secs);
            while Instant::now() < deadline {
                sleep(Duration::from_millis(200)).await;
                if let Some(state) = self.state.as_mut() {
                    terminal.draw(|f| ui::render(f, state))?;
                }
            }

            match connect(&self.url, &self.token).await {
                Ok(connected) => {
                    info!(attempt, "reconnected");
                    // Preserve the message store and session list; they're
                    // survivors of the drop. The daemon will re-send
                    // scrollback on re-attach anyway.
                    let previously_attached: Vec<String> = self
                        .state
                        .as_ref()
                        .map(|s| s.attached.iter().cloned().collect())
                        .unwrap_or_default();
                    self.absorb_connection(connected);
                    self.on_connected().await;
                    // Re-attach sequentially so the next send lands in
                    // the right broadcast list.
                    for id in previously_attached {
                        self.attach_if_needed(id).await;
                    }
                    return Ok(());
                }
                Err(e) => {
                    warn!(attempt, error = %e, "reconnect failed");
                }
            }
        }
        Err(anyhow!(
            "could not reconnect after {MAX_RECONNECT_ATTEMPTS} attempts"
        ))
    }

    // -------- user-driven commands --------

    async fn submit_prompt(&mut self) {
        let text = {
            let Some(state) = self.state.as_mut() else { return };
            let Some(text) = state.take_prompt() else {
                return;
            };
            text
        };

        // Slash-command shortcut — intercept before treating as a message.
        match commands::parse(&text) {
            Ok(Some(cmd)) => {
                self.dispatch_slash_command(cmd).await;
                return;
            }
            Err(e) => {
                if let Some(state) = self.state.as_mut() {
                    state.record_error(e.to_string());
                }
                return;
            }
            Ok(None) => {}
        }

        // Regular message send.
        let Some(session) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused().cloned())
        else {
            if let Some(state) = self.state.as_mut() {
                state.record_error("no session — try /new <name> [workdir]");
            }
            return;
        };

        // Serialize attach -> send on the same ordered mpsc. This is the
        // ONLY place that guarantees the daemon sees our attach before the
        // first send — without it, the daemon drops our broadcast list
        // for this session and the user sees nothing.
        self.attach_if_needed(session.id.clone()).await;

        let Some(handle) = self.handle.clone() else { return };
        let id = ClientHandle::next_request_id();
        let msg = ClientMessage::SessionSend {
            id,
            session_id: session.id,
            text,
            attachments: None,
            priority: None,
        };
        if let Err(e) = handle.send(msg).await {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("send failed: {e}"));
            }
            return;
        }
        if let Some(state) = self.state.as_mut() {
            state.last_error = None;
            state.scroll_to_bottom();
            state.focus = Focus::Prompt;
        }
    }

    async fn dispatch_slash_command(&mut self, cmd: SlashCommand) {
        match cmd {
            SlashCommand::New { name, workdir } => self.create_session(name, workdir).await,
            SlashCommand::Rename { name } => self.rename_focused(name).await,
            SlashCommand::Destroy => self.destroy_focused().await,
            SlashCommand::Interrupt => self.interrupt().await,
            SlashCommand::Approve => self.approve(true).await,
            SlashCommand::Deny => self.approve(false).await,
            SlashCommand::Rotate => self.rotate_focused().await,
            SlashCommand::SetMode(mode) => self.set_mode(mode).await,
            SlashCommand::Help => {
                if let Some(state) = self.state.as_mut() {
                    state.modal = Some(Modal::Help);
                }
            }
            SlashCommand::Clear => {
                // take_prompt already drained the editor; nothing else to do.
            }
        }
    }

    async fn rename_focused(&mut self, name: String) {
        let Some(session_id) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string))
        else {
            if let Some(state) = self.state.as_mut() {
                state.record_error("no session focused — nothing to rename");
            }
            return;
        };
        let Some(handle) = self.handle.clone() else { return };
        let id = ClientHandle::next_request_id();
        let msg = ClientMessage::SessionRename {
            id,
            session_id,
            name,
        };
        if let Err(e) = handle.send(msg).await {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("rename failed: {e}"));
            }
        }
        // No need to refresh the list — daemon broadcasts session.info_update
        // which our reducer already merges into AppState.sessions.
    }

    async fn create_session(&mut self, name: String, workdir: Option<String>) {
        let resolved_workdir = workdir
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "/tmp".into());

        let Some(handle) = self.handle.clone() else { return };
        let id = ClientHandle::next_request_id();
        let msg = ClientMessage::SessionCreate {
            id,
            name: name.clone(),
            workdir: resolved_workdir.clone(),
        };
        if let Err(e) = handle.send(msg).await {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("create-session failed: {e}"));
            }
            return;
        }
        // Refresh the session list so the new tab shows up.
        let list_id = ClientHandle::next_request_id();
        let _ = handle.send(ClientMessage::SessionList { id: list_id }).await;
        if let Some(state) = self.state.as_mut() {
            state.last_error = None;
        }
        info!(%name, workdir = %resolved_workdir, "requested session.create");
    }

    async fn destroy_focused(&mut self) {
        let Some(session_id) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string))
        else {
            return;
        };
        let Some(handle) = self.handle.clone() else { return };
        let id = ClientHandle::next_request_id();
        let _ = handle
            .send(ClientMessage::SessionDestroy { id, session_id })
            .await;
        let list_id = ClientHandle::next_request_id();
        let _ = handle.send(ClientMessage::SessionList { id: list_id }).await;
    }

    async fn rotate_focused(&mut self) {
        let Some(session_id) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string))
        else {
            return;
        };
        let Some(handle) = self.handle.clone() else { return };
        let id = ClientHandle::next_request_id();
        let _ = handle
            .send(ClientMessage::SessionRotate { id, session_id })
            .await;
    }

    async fn set_mode(&mut self, mode: SessionMode) {
        let Some(session_id) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string))
        else {
            return;
        };
        let Some(handle) = self.handle.clone() else { return };
        let id = ClientHandle::next_request_id();
        let _ = handle
            .send(ClientMessage::SessionSetMode {
                id,
                session_id,
                mode,
                max_turns: None,
            })
            .await;
    }

    async fn ensure_attached(&mut self) {
        let Some(state) = self.state.as_ref() else { return };
        let Some(id) = state.sessions.focused_id().map(ToString::to_string) else {
            return;
        };
        self.attach_if_needed(id).await;
    }

    /// Ensure `session.attach` has been enqueued to the daemon for this
    /// session, serially — awaits the mpsc write so any subsequent send
    /// is guaranteed to arrive after the attach. Idempotent via
    /// `note_attached`.
    ///
    /// Previously this used `tokio::spawn` which introduced a race: the
    /// spawned task wasn't guaranteed to enqueue the attach before the
    /// caller's next `handle.send(...)` call did, so the first
    /// `session.send` after a switch could arrive at the daemon before
    /// the attach and be silently dropped from this client's broadcast
    /// list.
    async fn attach_if_needed(&mut self, session_id: String) {
        {
            let Some(state) = self.state.as_mut() else { return };
            if !state.note_attached(&session_id) {
                return;
            }
        }
        let Some(handle) = self.handle.clone() else { return };
        let msg = ClientMessage::SessionAttach {
            id: ClientHandle::next_request_id(),
            session_id: session_id.clone(),
        };
        if let Err(e) = handle.send(msg).await {
            warn!(error = %e, session_id = %session_id, "session.attach failed");
            // Roll back the ledger so a retry can try again.
            if let Some(state) = self.state.as_mut() {
                state.attached.remove(&session_id);
            }
        }
    }

    async fn interrupt(&mut self) {
        let Some(state) = self.state.as_ref() else { return };
        let Some(session) = state.sessions.focused() else { return };
        let sid = session.id.clone();
        let Some(handle) = self.handle.clone() else { return };
        let id = ClientHandle::next_request_id();
        if let Err(e) = handle
            .send(ClientMessage::SessionInterrupt {
                id,
                session_id: sid,
            })
            .await
        {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("interrupt failed: {e}"));
            }
        }
    }

    async fn approve(&mut self, approved: bool) {
        let (session_id, approval_id) = {
            let Some(state) = self.state.as_ref() else { return };
            let Some(session) = state.sessions.focused() else { return };
            let sid = session.id.clone();
            let Some(approval_id) = find_latest_approval(state, &sid) else {
                return;
            };
            (sid, approval_id)
        };
        let Some(handle) = self.handle.clone() else { return };
        let id = ClientHandle::next_request_id();
        if let Err(e) = handle
            .send(ClientMessage::SessionApprove {
                id,
                session_id,
                approval_id,
                approved,
            })
            .await
        {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("approval failed: {e}"));
            }
        }
    }
}

/// Tab-autocomplete in command mode: if the user's partial command
/// uniquely matches a catalog entry, replace the prompt with
/// `/<full-name> ` (trailing space so they can start typing args). No-op
/// on ambiguous or zero matches — the palette hint line already tells the
/// user what their options are.
fn autocomplete_command(state: &mut AppState) {
    use tui_textarea::TextArea;

    let Some(query) = state.command_query() else { return };
    let Some(full) = commands::unique_completion(query) else {
        return;
    };

    // Rebuild the editor with the completed command + trailing space.
    let mut fresh = TextArea::default();
    fresh.set_cursor_line_style(ratatui::style::Style::default());
    fresh.set_placeholder_text("Message…  Enter sends · Shift+Enter newline · Esc blurs");
    fresh.insert_str(format!("/{full} "));
    state.prompt = fresh;
}

/// PageUp/PageDown step: a viewport-minus-one. Standard pager UX
/// (more, less, vim) keeps one row of overlap on each page so the
/// reader doesn't lose their place. Falls back to a sane default
/// before the first frame, when we don't yet know the viewport size.
fn page_step(state: &AppState) -> u16 {
    state.last_viewport_rows.saturating_sub(1).max(10)
}

/// Returns `true` if the app's visual state will change on the next tick —
/// i.e., there's an animation in progress that the user expects to see
/// move. If `false`, the event loop can safely skip the redraw, which
/// lets native terminal text selection stick (ratatui repaints every
/// cell on draw, clobbering any selection highlight).
fn needs_animation_frame(state: &AppState) -> bool {
    // Connection pill animates during reconnect.
    if matches!(state.connection, ConnectionState::Reconnecting { .. }) {
        return true;
    }

    let Some(session) = state.sessions.focused() else {
        return false;
    };

    // Session-level busy signal.
    if matches!(session.status, SessionStatus::Working) {
        return true;
    }

    // Any tool in a phase that shows a spinner.
    let msgs = state.messages.messages(&session.id);
    let any_tool_running = msgs.iter().any(|m| {
        m.tool.as_ref().is_some_and(|t| {
            matches!(
                &t.state,
                ToolState::Streaming { .. } | ToolState::Executing { .. }
            )
        })
    });
    if any_tool_running {
        return true;
    }

    // Recent streaming delta still within the fallback window used by
    // the worker row.
    if state
        .ticks_since_activity(&session.id)
        .is_some_and(|t| t < 20)
    {
        return true;
    }

    false
}

fn find_latest_approval(state: &AppState, session_id: &str) -> Option<String> {
    state
        .messages
        .messages(session_id)
        .iter()
        .rev()
        .filter_map(|m| {
            let tool = m.tool.as_ref()?;
            match &tool.state {
                codeoid_protocol::ToolState::WaitingConfirmation { approval_id, .. } => {
                    Some(approval_id.clone())
                }
                _ => None,
            }
        })
        .next()
}

