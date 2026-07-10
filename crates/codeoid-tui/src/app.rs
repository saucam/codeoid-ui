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
    ClientMessage, DaemonMessage, SessionInfo, SessionMode, SessionStatus, ToolState,
};
use crossterm::event::{Event as CtEvent, EventStream, KeyEventKind, MouseEventKind};
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

/// Cap on messages buffered while disconnected, so a long outage can't
/// grow the queue without bound. Matches the web client's `MAX_QUEUED`.
const MAX_QUEUED_SENDS: usize = 200;

pub struct App {
    url: String,
    token: String,
    state: Option<AppState>,
    handle: Option<ClientHandle>,
    daemon_events: Option<mpsc::Receiver<StreamEvent>>,
    quit_requested: bool,
    /// Outbound user messages typed while the socket was down. Flushed in
    /// order on reconnect so a message composed during a blip is never
    /// silently lost (mirrors `pendingSends` in the web client).
    pending_sends: Vec<ClientMessage>,
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
            pending_sends: Vec::new(),
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
                self.state.as_ref().is_some_and(needs_animation_frame)
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
                let modal_kind = match state.modal.as_ref() {
                    Some(crate::state::Modal::AskUserQuestion(_)) => {
                        crate::keymap::ModalKind::AskUserQuestion
                    }
                    Some(crate::state::Modal::UiDialog(m)) if m.is_text_entry() => {
                        crate::keymap::ModalKind::UiDialogText
                    }
                    Some(crate::state::Modal::UiDialog(_)) => {
                        crate::keymap::ModalKind::UiDialogChoice
                    }
                    Some(_) => crate::keymap::ModalKind::Generic,
                    None => crate::keymap::ModalKind::None,
                };
                let modal_open = !matches!(modal_kind, crate::keymap::ModalKind::None);
                let command_mode = state.is_command_mode();

                // Esc interrupts a busy focused session — Claude Code parity.
                // This binding is runtime-conditional (depends on live session
                // status), which the static keymap table can't express, so it's
                // resolved here ahead of resolve()'s context-free Esc (blur
                // prompt in prompt mode / no-op in nav). When a modal is open we
                // defer to resolve() so Esc closes the modal first. Ctrl+X and
                // `.` remain the unconditional interrupt aliases in the keymap.
                let esc_interrupts = matches!(key.code, crossterm::event::KeyCode::Esc)
                    && !modal_open
                    && state.sessions.focused().is_some_and(|s| {
                        matches!(
                            s.status,
                            SessionStatus::Working | SessionStatus::WaitingApproval
                        )
                    });
                let action = if esc_interrupts {
                    Some(crate::keymap::Action::Interrupt)
                } else {
                    resolve(key, prompt_focused, modal_kind, command_mode)
                };

                if let Some(action) = action {
                    self.apply_action(action).await;
                } else if matches!(modal_kind, crate::keymap::ModalKind::UiDialogText) {
                    // Unbound keys edit the dialog's text buffer.
                    if let Some(Modal::UiDialog(m)) = state.modal.as_mut() {
                        match key.code {
                            crossterm::event::KeyCode::Char(c) => m.buffer.push(c),
                            crossterm::event::KeyCode::Backspace => {
                                m.buffer.pop();
                            }
                            _ => {}
                        }
                    }
                } else if prompt_focused && !modal_open {
                    // Anything keymap didn't handle goes straight to the
                    // TextArea — arrows, Home/End, Ctrl+A/E, word-delete,
                    // selection, paste, the lot.
                    state.prompt.input(key);
                }
            }
            AppEvent::Terminal(CtEvent::Paste(text)) => {
                if let Some(Modal::UiDialog(m)) = state.modal.as_mut() {
                    if m.is_text_entry() {
                        m.buffer.push_str(&text);
                    }
                } else if state.focus == Focus::Prompt && state.modal.is_none() {
                    state.prompt.insert_str(&text);
                }
            }
            AppEvent::Terminal(CtEvent::Mouse(m)) => {
                // Wheel scrolls the transcript regardless of which pane
                // owns keyboard focus. 3 rows per notch matches the
                // convention most modern terminals use for line-mode wheel.
                match m.kind {
                    MouseEventKind::ScrollUp => state.scroll_up(3),
                    MouseEventKind::ScrollDown => state.scroll_down(3),
                    _ => {}
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
        let Some(state) = self.state.as_mut() else {
            return;
        };
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
                self.on_focus_changed().await;
            }
            Action::PrevSession => {
                state.sessions.focus_prev();
                state.scroll_to_bottom();
                self.ensure_attached().await;
                self.on_focus_changed().await;
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
            Action::ToggleVerboseToolOutput => {
                state.verbose_tool_output = !state.verbose_tool_output;
                // Tool blocks are cached per-message at the render layer;
                // flipping verbose changes their height, so blow the
                // caches so the next frame rebuilds at the new size.
                state.render_cache.clear();
                state.scrollback_build.clear();
            }
            Action::SelectNextToolBlock => state.cycle_tool_block_selection(true),
            Action::SelectPrevToolBlock => state.cycle_tool_block_selection(false),
            Action::ToggleExpandSelectedToolBlock => state.toggle_expand_selected_tool_block(),
            Action::AskToggleOption(n) => {
                if let Some(Modal::AskUserQuestion(m)) = state.modal.as_mut() {
                    if n >= 1 {
                        m.toggle_option((n - 1) as usize);
                    }
                }
            }
            Action::AskNextQuestion => {
                if let Some(Modal::AskUserQuestion(m)) = state.modal.as_mut() {
                    m.next_question();
                }
            }
            Action::AskPrevQuestion => {
                if let Some(Modal::AskUserQuestion(m)) = state.modal.as_mut() {
                    m.prev_question();
                }
            }
            Action::AskSubmit => self.submit_ask_user_question().await,
            Action::AskCancel => self.cancel_ask_user_question().await,
            Action::UiDialogNext => {
                if let Some(Modal::UiDialog(m)) = state.modal.as_mut() {
                    m.next_option();
                }
            }
            Action::UiDialogPrev => {
                if let Some(Modal::UiDialog(m)) = state.modal.as_mut() {
                    m.prev_option();
                }
            }
            Action::UiDialogPick(n) => {
                let picked = if let Some(Modal::UiDialog(m)) = state.modal.as_mut() {
                    let len = m.request.options.as_ref().map_or(0, Vec::len);
                    let idx = (n as usize).saturating_sub(1);
                    if idx < len {
                        m.selected = idx;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if picked {
                    self.submit_ui_dialog().await;
                }
            }
            Action::UiDialogYes => self.answer_ui_dialog_confirm(true).await,
            Action::UiDialogNo => self.answer_ui_dialog_confirm(false).await,
            Action::UiDialogSubmit => self.submit_ui_dialog().await,
            Action::UiDialogCancel => self.cancel_ui_dialog().await,
        }
    }

    /// Submit the open UiDialog with its method-appropriate payload.
    async fn submit_ui_dialog(&mut self) {
        use codeoid_protocol::UiRequestMethod;
        let payload = {
            let Some(state) = self.state.as_ref() else {
                return;
            };
            let Some(Modal::UiDialog(m)) = state.modal.as_ref() else {
                return;
            };
            match m.request.method {
                UiRequestMethod::Select => {
                    let Some(value) = m.selected_option() else {
                        return; // empty options list — nothing to submit
                    };
                    (Some(value.to_string()), None)
                }
                UiRequestMethod::Confirm => (None, Some(true)),
                UiRequestMethod::Input | UiRequestMethod::Editor => (Some(m.buffer.clone()), None),
            }
        };
        self.respond_ui_dialog(payload.0, payload.1, false).await;
    }

    /// Answer a confirm dialog (`y` / `n`). No-op for other methods so the
    /// keys can't accidentally answer a select.
    async fn answer_ui_dialog_confirm(&mut self, confirmed: bool) {
        use codeoid_protocol::UiRequestMethod;
        let is_confirm = self.state.as_ref().is_some_and(|s| {
            matches!(
                s.modal.as_ref(),
                Some(Modal::UiDialog(m)) if m.request.method == UiRequestMethod::Confirm
            )
        });
        if is_confirm {
            self.respond_ui_dialog(None, Some(confirmed), false).await;
        }
    }

    async fn cancel_ui_dialog(&mut self) {
        self.respond_ui_dialog(None, None, true).await;
    }

    /// Ship a `session.ui_response` for the open dialog, close the modal,
    /// and surface the next pending dialog (if any). The daemon's
    /// `session.ui_resolved` broadcast is the authoritative cleanup for the
    /// pending list — we drop our copy optimistically.
    async fn respond_ui_dialog(
        &mut self,
        value: Option<String>,
        confirmed: Option<bool>,
        cancelled: bool,
    ) {
        let ids = {
            let Some(state) = self.state.as_mut() else {
                return;
            };
            let Some(Modal::UiDialog(m)) = state.modal.as_ref() else {
                return;
            };
            let ids = (m.request.session_id.clone(), m.request.request_id.clone());
            state.remove_ui_request(&ids.0, &ids.1);
            state.maybe_open_ui_dialog();
            ids
        };
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let (session_id, request_id) = ids;
        let id = ClientHandle::next_request_id();
        if let Err(e) = handle
            .send(ClientMessage::SessionUiResponse {
                id,
                session_id,
                request_id,
                value,
                confirmed,
                cancelled: cancelled.then_some(true),
            })
            .await
        {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("dialog response failed: {e}"));
            }
        }
    }

    /// Focus moved to a (possibly different) session: fetch its provider
    /// commands and surface its oldest pending dialog if no modal is up.
    async fn on_focus_changed(&mut self) {
        self.maybe_fetch_commands().await;
        if let Some(state) = self.state.as_mut() {
            state.maybe_open_ui_dialog();
        }
    }

    /// Fetch the focused session's provider-command catalog once per
    /// connection. Older daemons reject the verb — the error path records
    /// nothing and the catalog stays empty.
    async fn maybe_fetch_commands(&mut self) {
        let request = {
            let Some(state) = self.state.as_mut() else {
                return;
            };
            let Some(session_id) = state.sessions.focused().map(|s| s.id.clone()) else {
                return;
            };
            if !state.commands_requested.insert(session_id.clone()) {
                return;
            }
            session_id
        };
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let id = ClientHandle::next_request_id();
        let _ = handle
            .send(ClientMessage::SessionCommands {
                id,
                session_id: request,
            })
            .await;
    }

    async fn submit_ask_user_question(&mut self) {
        let payload = {
            let Some(state) = self.state.as_mut() else {
                return;
            };
            let Some(Modal::AskUserQuestion(m)) = state.modal.as_ref() else {
                return;
            };
            if !m.all_answered() {
                return;
            }
            let answers = m.build_answers();
            let answers_value = serde_json::to_value(answers).ok();
            let Some(answers_value) = answers_value else {
                return;
            };
            let mut updated = serde_json::Map::new();
            updated.insert("answers".to_string(), answers_value);
            (
                m.session_id.clone(),
                m.approval_id.clone(),
                serde_json::Value::Object(updated),
            )
        };
        if let Some(state) = self.state.as_mut() {
            state.modal = None;
        }
        let (session_id, approval_id, updated_input) = payload;
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let id = ClientHandle::next_request_id();
        if let Err(e) = handle
            .send(ClientMessage::SessionApprove {
                id,
                session_id,
                approval_id,
                approved: true,
                updated_input: Some(updated_input),
            })
            .await
        {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("ask submit failed: {e}"));
            }
        }
    }

    async fn cancel_ask_user_question(&mut self) {
        let payload = {
            let Some(state) = self.state.as_mut() else {
                return;
            };
            let Some(Modal::AskUserQuestion(m)) = state.modal.as_ref() else {
                return;
            };
            (m.session_id.clone(), m.approval_id.clone())
        };
        if let Some(state) = self.state.as_mut() {
            state.modal = None;
        }
        let (session_id, approval_id) = payload;
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let id = ClientHandle::next_request_id();
        let _ = handle
            .send(ClientMessage::SessionApprove {
                id,
                session_id,
                approval_id,
                approved: false,
                updated_input: None,
            })
            .await;
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
            DaemonMessage::ModelsListResult { models, live, .. } => {
                state.models = models;
                state.models_live = live;
            }
            DaemonMessage::SessionInfoUpdate { session, .. } => {
                state.merge_session(session);
            }
            DaemonMessage::SessionStatusChange {
                session_id, status, ..
            } => {
                // Newest sessions live at the tail — search from the back
                // (status flips overwhelmingly target recent sessions).
                if let Some(s) = state
                    .sessions
                    .items()
                    .iter()
                    .rev()
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
            DaemonMessage::SessionExportResult {
                manifest, payload, ..
            } => {
                let summary = match &payload {
                    codeoid_protocol::SessionExportPayload::File { path, size_bytes } => {
                        format!(
                            "exported · alias={} · {} msgs/{} ep/{} turns · {} ({} KB)",
                            manifest.workdir.alias,
                            manifest.counts.messages,
                            manifest.counts.episodes,
                            manifest.counts.turns,
                            path,
                            size_bytes / 1024
                        )
                    }
                    codeoid_protocol::SessionExportPayload::Inline { size_bytes, .. } => {
                        format!(
                            "exported · alias={} · {} msgs/{} ep/{} turns · inline ({} KB)",
                            manifest.workdir.alias,
                            manifest.counts.messages,
                            manifest.counts.episodes,
                            manifest.counts.turns,
                            size_bytes / 1024
                        )
                    }
                };
                state.record_error(summary); // surfaced in the footer
            }
            DaemonMessage::SessionImportResult {
                new_session_id,
                imported_messages,
                imported_episodes,
                imported_turns,
                pinned_files_written,
                warnings,
                ..
            } => {
                let summary = format!(
                    "imported session {} · {} msgs · {} ep · {} turns · {} pinned · {} warnings",
                    &new_session_id[..8.min(new_session_id.len())],
                    imported_messages,
                    imported_episodes,
                    imported_turns,
                    pinned_files_written,
                    warnings.len()
                );
                state.record_error(summary);
                // Refresh session list so the new entry appears.
                if let Some(handle) = self.handle.clone() {
                    tokio::spawn(async move {
                        let _ = handle
                            .send(ClientMessage::SessionList {
                                id: ClientHandle::next_request_id(),
                            })
                            .await;
                    });
                }
            }
            DaemonMessage::ClaudeConfigResult {
                request_id,
                workdir,
                agents,
                skills,
                mcp_servers,
                hooks,
            } => {
                if let Some(Modal::Capabilities(m)) = state.modal.as_mut() {
                    // Drop stale results — user may have closed the modal
                    // and reopened with a different request id.
                    if m.pending_request_id.as_deref() == Some(request_id.as_str()) {
                        m.loading = false;
                        m.error = None;
                        m.workdir = Some(workdir);
                        m.agents = agents;
                        m.skills = skills;
                        m.mcp_servers = mcp_servers;
                        m.hooks = hooks;
                        m.pending_request_id = None;
                        m.selected = 0;
                    }
                }
            }
            DaemonMessage::SessionUiRequest(req) => {
                state.add_ui_request(req);
                state.maybe_open_ui_dialog();
            }
            DaemonMessage::SessionUiResolved {
                session_id,
                request_id,
                ..
            } => {
                // Authoritative dismiss — fires whether WE answered, another
                // client did, the request timed out, or the turn was
                // interrupted. Surface the next pending dialog if any.
                state.remove_ui_request(&session_id, &request_id);
                state.maybe_open_ui_dialog();
            }
            DaemonMessage::SessionCommandsResult {
                session_id,
                commands,
                ..
            } => {
                debug!(
                    session = %session_id,
                    count = commands.len(),
                    "provider command catalog received"
                );
                state.provider_commands.insert(session_id, commands);
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
        // Same for the fetch-once command ledger and pending dialogs: the
        // daemon re-sends pending ui_requests on attach, and catalogs may
        // have changed while we were away.
        state.commands_requested.clear();
        state.pending_ui_requests.clear();
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
            // Fetch the model catalog so /model can validate + display.
            // Fire-and-forget: the result routes back through apply_daemon.
            let mid = ClientHandle::next_request_id();
            if let Err(e) = handle.send(ClientMessage::ModelsList { id: mid }).await {
                warn!(error = %e, "failed to send models.list");
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
                    // Drain anything the user typed while we were down.
                    self.flush_pending_sends().await;
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
            let Some(state) = self.state.as_mut() else {
                return;
            };
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
            Err(commands::ParseError::Unknown(verb))
                if self
                    .state
                    .as_ref()
                    .is_some_and(|s| s.is_provider_command(&verb)) =>
            {
                // Provider command (pi extension / prompt template / skill,
                // from the session.commands catalog): not a client verb —
                // fall through and send the raw text so the provider
                // expands it.
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

        self.send_user_text(session.id, text).await;
    }

    /// Send a user message now, or buffer it for flush-on-reconnect if the
    /// socket is down. Attach-before-send ordering is preserved when live
    /// (and re-applied on flush), so the daemon always has us in the
    /// session's broadcast list before the message lands.
    async fn send_user_text(&mut self, session_id: String, text: String) {
        let connected = matches!(
            self.state.as_ref().map(|s| &s.connection),
            Some(ConnectionState::Connected)
        );
        let msg = ClientMessage::SessionSend {
            id: ClientHandle::next_request_id(),
            session_id: session_id.clone(),
            text,
            attachments: None,
            priority: None,
        };

        if connected {
            // Serialize attach -> send on the same ordered mpsc so the daemon
            // sees our attach before the first send.
            self.attach_if_needed(session_id).await;
            if let Some(handle) = self.handle.clone() {
                if handle.send(msg.clone()).await.is_ok() {
                    if let Some(state) = self.state.as_mut() {
                        state.last_error = None;
                        state.scroll_to_bottom();
                        state.focus = Focus::Prompt;
                    }
                    return;
                }
            }
        }

        // Offline, or the send raced a drop — buffer it.
        self.queue_send(msg);
        if let Some(state) = self.state.as_mut() {
            state.record_error("offline — message queued, will send on reconnect");
            state.scroll_to_bottom();
            state.focus = Focus::Prompt;
        }
    }

    /// Buffer an outbound message (bounded). Silently drops past the cap —
    /// a minutes-long outage shouldn't grow memory without limit.
    fn queue_send(&mut self, msg: ClientMessage) {
        if self.pending_sends.len() < MAX_QUEUED_SENDS {
            self.pending_sends.push(msg);
        }
    }

    /// Flush buffered sends in order after a reconnect. Re-attaches before
    /// each session send. On a re-failure, the unsent remainder is requeued
    /// (order preserved) for the next reconnect.
    async fn flush_pending_sends(&mut self) {
        if self.pending_sends.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.pending_sends);
        let total = batch.len();
        for (i, msg) in batch.iter().enumerate() {
            if let ClientMessage::SessionSend { session_id, .. } = msg {
                self.attach_if_needed(session_id.clone()).await;
            }
            let Some(handle) = self.handle.clone() else {
                self.pending_sends.extend_from_slice(&batch[i..]);
                return;
            };
            if handle.send(msg.clone()).await.is_err() {
                self.pending_sends.extend_from_slice(&batch[i..]);
                return;
            }
        }
        if let Some(state) = self.state.as_mut() {
            state.record_error(format!("reconnected — flushed {total} queued message(s)"));
        }
    }

    async fn dispatch_slash_command(&mut self, cmd: SlashCommand) {
        match cmd {
            SlashCommand::New {
                name,
                workdir,
                provider_id,
            } => self.create_session(name, workdir, provider_id).await,
            SlashCommand::Provider(provider_id) => self.set_provider(provider_id).await,
            SlashCommand::Fork { provider_id } => self.fork_focused(provider_id).await,
            SlashCommand::Rename { name } => self.rename_focused(name).await,
            SlashCommand::Destroy => self.destroy_focused().await,
            SlashCommand::Interrupt => self.interrupt().await,
            SlashCommand::Approve => self.approve(true).await,
            SlashCommand::Deny => self.approve(false).await,
            SlashCommand::Rotate => self.rotate_focused().await,
            SlashCommand::SetMode(mode) => self.set_mode(mode).await,
            SlashCommand::Model(value) => self.set_model(value).await,
            SlashCommand::Who => self.show_who(),
            SlashCommand::Help => {
                if let Some(state) = self.state.as_mut() {
                    state.modal = Some(Modal::Help);
                }
            }
            SlashCommand::Clear => {
                // take_prompt already drained the editor; nothing else to do.
            }
            SlashCommand::Capabilities(tab) => {
                self.open_capabilities(tab).await;
            }
            SlashCommand::Export { path } => {
                self.export_focused(path).await;
            }
            SlashCommand::Import {
                bundle_path,
                target_workdir,
            } => {
                self.import_bundle(bundle_path, target_workdir).await;
            }
        }
    }

    async fn export_focused(&mut self, _path: Option<String>) {
        let session_id = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string));
        let Some(session_id) = session_id else {
            if let Some(state) = self.state.as_mut() {
                state.record_error("no session focused — nothing to export");
            }
            return;
        };
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let id = ClientHandle::next_request_id();
        let msg = ClientMessage::SessionExport {
            id,
            session_id,
            include_memory: Some(true),
            include_pinned_files: Some(false),
            alias_override: None,
            // Always force on-disk for the TUI — operators want a path
            // they can `scp` to a teammate; inline base64 in the
            // terminal is awkward.
            to_file: Some(true),
        };
        if let Err(e) = handle.send(msg).await {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("export failed: {e}"));
            }
        }
    }

    async fn import_bundle(&mut self, bundle_path: String, target_workdir: String) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let id = ClientHandle::next_request_id();
        let msg = ClientMessage::SessionImport {
            id,
            source: codeoid_protocol::SessionImportSource::File { path: bundle_path },
            target_workdir,
            name_override: None,
            write_pinned_files: Some(false),
        };
        if let Err(e) = handle.send(msg).await {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("import failed: {e}"));
            }
        }
    }

    async fn open_capabilities(&mut self, tab: crate::commands::CapabilitiesTab) {
        let session_id = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string));
        let Some(session_id) = session_id else {
            if let Some(state) = self.state.as_mut() {
                state.record_error("no session focused — capabilities need a session");
            }
            return;
        };
        let modal_tab = match tab {
            crate::commands::CapabilitiesTab::Agents => crate::state::CapabilitiesTab::Agents,
            crate::commands::CapabilitiesTab::Skills => crate::state::CapabilitiesTab::Skills,
            crate::commands::CapabilitiesTab::Mcp => crate::state::CapabilitiesTab::Mcp,
            crate::commands::CapabilitiesTab::Hooks => crate::state::CapabilitiesTab::Hooks,
        };
        let request_id = ClientHandle::next_request_id();
        if let Some(state) = self.state.as_mut() {
            let mut modal = crate::state::CapabilitiesModal::new(modal_tab);
            modal.pending_request_id = Some(request_id.clone());
            state.modal = Some(Modal::Capabilities(modal));
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let msg = ClientMessage::ClaudeConfig {
            id: request_id,
            session_id,
        };
        if let Err(e) = handle.send(msg).await {
            if let Some(state) = self.state.as_mut() {
                if let Some(Modal::Capabilities(m)) = state.modal.as_mut() {
                    m.loading = false;
                    m.error = Some(format!("failed to send: {e}"));
                }
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
        let Some(handle) = self.handle.clone() else {
            return;
        };
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

    async fn create_session(
        &mut self,
        name: String,
        workdir: Option<String>,
        provider_id: Option<String>,
    ) {
        let resolved_workdir = workdir
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "/tmp".into());

        let Some(handle) = self.handle.clone() else {
            return;
        };
        let msg = session_create_message(
            ClientHandle::next_request_id(),
            name.clone(),
            resolved_workdir.clone(),
            provider_id,
        );
        if let Err(e) = handle.send(msg).await {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("create-session failed: {e}"));
            }
            return;
        }
        // Refresh the session list so the new tab shows up.
        let list_id = ClientHandle::next_request_id();
        let _ = handle
            .send(ClientMessage::SessionList { id: list_id })
            .await;
        if let Some(state) = self.state.as_mut() {
            state.last_error = None;
        }
        info!(%name, workdir = %resolved_workdir, "requested session.create");
    }

    /// `/provider <id>` — switch the focused session's backend. The daemon
    /// validates fail-closed (unknown id, mid-turn) and its error lands via
    /// the response; the switch announcement arrives as an info message.
    async fn set_provider(&mut self, provider_id: String) {
        let Some(session_id) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string))
        else {
            if let Some(state) = self.state.as_mut() {
                state.record_error("no session focused — /provider needs one");
            }
            return;
        };
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let msg = set_provider_message(
            ClientHandle::next_request_id(),
            session_id,
            provider_id.clone(),
        );
        match handle.request_ok(msg).await {
            Ok(_) => {
                if let Some(state) = self.state.as_mut() {
                    state.last_error = None;
                }
            }
            Err(e) => {
                // Daemon rejections (mid-turn switch, unknown provider)
                // surface here with their real message.
                if let Some(state) = self.state.as_mut() {
                    state.record_error(format!("/provider failed: {e}"));
                }
            }
        }
    }

    /// `/fork [backend]` — branch the focused session into an independent one,
    /// optionally continuing it on another backend. The daemon returns the new
    /// [`SessionInfo`]; we upsert it and focus it so the new tab is active. It
    /// validates fail-closed (unknown session/backend) and its error lands via
    /// the response.
    async fn fork_focused(&mut self, provider_id: Option<String>) {
        let Some(session_id) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string))
        else {
            if let Some(state) = self.state.as_mut() {
                state.record_error("no session focused — /fork needs one");
            }
            return;
        };
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let msg = session_fork_message(ClientHandle::next_request_id(), session_id, provider_id);
        match handle.request_ok(msg).await {
            Ok(data) => {
                let applied = self
                    .state
                    .as_mut()
                    .map(|state| apply_fork_response(state, data));
                match applied {
                    // Fork landed: refresh the list so the daemon's view and
                    // ours reconcile (the fork isn't attached, so nothing
                    // arrives on its own).
                    Some(Ok(())) => {
                        let _ = handle
                            .send(ClientMessage::SessionList {
                                id: ClientHandle::next_request_id(),
                            })
                            .await;
                    }
                    Some(Err(e)) => {
                        if let Some(state) = self.state.as_mut() {
                            state.record_error(e);
                        }
                    }
                    None => {}
                }
            }
            Err(e) => {
                if let Some(state) = self.state.as_mut() {
                    state.record_error(format!("/fork failed: {e}"));
                }
            }
        }
    }

    async fn destroy_focused(&mut self) {
        let Some(session_id) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string))
        else {
            return;
        };
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let id = ClientHandle::next_request_id();
        let _ = handle
            .send(ClientMessage::SessionDestroy { id, session_id })
            .await;
        let list_id = ClientHandle::next_request_id();
        let _ = handle
            .send(ClientMessage::SessionList { id: list_id })
            .await;
    }

    async fn rotate_focused(&mut self) {
        let Some(session_id) = self
            .state
            .as_ref()
            .and_then(|s| s.sessions.focused_id().map(ToString::to_string))
        else {
            return;
        };
        let Some(handle) = self.handle.clone() else {
            return;
        };
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
        let Some(handle) = self.handle.clone() else {
            return;
        };
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

    /// `/model` with no arg — list the catalog (and mark the current +
    /// default) in the footer.
    fn list_models(&mut self) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if state.models.is_empty() {
            state.record_error("models: catalog not loaded yet — try again in a moment");
            return;
        }
        let current = state.sessions.focused().and_then(|s| s.model.clone());
        let list = state
            .models
            .iter()
            .map(|m| {
                let mark = if current.as_deref() == Some(m.value.as_str()) {
                    "● " // current
                } else if m.is_default.unwrap_or(false) {
                    "★ " // backend default
                } else {
                    ""
                };
                format!("{mark}{}", m.value)
            })
            .collect::<Vec<_>>()
            .join("  ·  ");
        let src = if state.models_live {
            "live"
        } else {
            "fallback"
        };
        state.record_error(format!(
            "models ({src}): {list}   — /model <value> to switch"
        ));
    }

    /// `/model <value>` — validate against the catalog, then switch the
    /// focused session's model. Mirrors the web/Telegram feedback on an
    /// invalid model.
    async fn set_model(&mut self, value: Option<String>) {
        let Some(value) = value else {
            self.list_models();
            return;
        };

        // Decide what to do under a single scoped borrow, so we don't hold
        // an immutable borrow of `state` across a later `as_mut()`.
        enum Plan {
            Send(String),
            Invalid(String),
            NoSession,
        }
        let plan = {
            let Some(state) = self.state.as_ref() else {
                return;
            };
            if !state.models.is_empty() && !state.models.iter().any(|m| m.value == value) {
                let valid = state
                    .models
                    .iter()
                    .map(|m| m.value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Plan::Invalid(format!("model '{value}' not found — available: {valid}"))
            } else if let Some(id) = state.sessions.focused_id().map(ToString::to_string) {
                Plan::Send(id)
            } else {
                Plan::NoSession
            }
        };

        let session_id = match plan {
            Plan::Send(id) => id,
            Plan::Invalid(msg) => {
                if let Some(s) = self.state.as_mut() {
                    s.record_error(msg);
                }
                return;
            }
            Plan::NoSession => {
                if let Some(s) = self.state.as_mut() {
                    s.record_error("no session focused — /model needs a session");
                }
                return;
            }
        };

        let Some(handle) = self.handle.clone() else {
            return;
        };
        let id = ClientHandle::next_request_id();
        if let Err(e) = handle
            .send(ClientMessage::SessionSetModel {
                id,
                session_id,
                model: value.clone(),
                fallback_model: None,
            })
            .await
        {
            if let Some(state) = self.state.as_mut() {
                state.record_error(format!("set-model failed: {e}"));
            }
        } else if let Some(state) = self.state.as_mut() {
            let disp = state.model_display(&value);
            state.record_error(format!("model → {disp}"));
        }
    }

    /// `/who` — surface the authenticated ZeroID identity + scope count.
    fn show_who(&mut self) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let sub = state.auth.identity.sub.clone();
        let name = state
            .auth
            .identity
            .name
            .clone()
            .unwrap_or_else(|| "—".to_string());
        let scopes = state.auth.scopes.len();
        state.record_error(format!("you: {sub} · {name} · {scopes} scopes"));
    }

    async fn ensure_attached(&mut self) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(id) = state.sessions.focused_id().map(ToString::to_string) else {
            return;
        };
        self.attach_if_needed(id).await;
        // Attach is the "user is now looking at this session" signal — the
        // right moment to (once) pull its provider-command catalog.
        self.maybe_fetch_commands().await;
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
            let Some(state) = self.state.as_mut() else {
                return;
            };
            if !state.note_attached(&session_id) {
                return;
            }
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
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
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(session) = state.sessions.focused() else {
            return;
        };
        let sid = session.id.clone();
        let Some(handle) = self.handle.clone() else {
            return;
        };
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
            let Some(state) = self.state.as_ref() else {
                return;
            };
            let Some(session) = state.sessions.focused() else {
                return;
            };
            let sid = session.id.clone();
            let Some(approval_id) = find_latest_approval(state, &sid) else {
                return;
            };
            (sid, approval_id)
        };
        // AskUserQuestion needs an answer payload, not a binary y/n.
        // Approve (`y`) on it pops the question form modal instead of
        // sending a bare allow that would arrive as `answers: {}`.
        // Deny (`d`) goes through the normal path — Claude sees a denial
        // just like it would for any other tool.
        if approved {
            if let Some(state) = self.state.as_mut() {
                if let Some(modal) = build_ask_user_question_modal(state, &session_id, &approval_id)
                {
                    state.modal = Some(Modal::AskUserQuestion(modal));
                    return;
                }
            }
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let id = ClientHandle::next_request_id();
        if let Err(e) = handle
            .send(ClientMessage::SessionApprove {
                id,
                session_id,
                approval_id,
                approved,
                updated_input: None,
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

    let Some(query) = state.command_query() else {
        return;
    };
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
fn page_step(state: &AppState) -> usize {
    usize::from(state.last_viewport_rows.saturating_sub(1).max(10))
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

/// If the focused session's pending approval is for `AskUserQuestion`,
/// extract the question list out of its tool input so we can show a
/// form modal. Returns `None` for any other tool — the caller falls
/// through to the binary approve path.
fn build_ask_user_question_modal(
    state: &AppState,
    session_id: &str,
    approval_id: &str,
) -> Option<crate::state::AskUserQuestionModal> {
    use crate::state::{AskOption, AskUserQuestionModal, AskUserQuestionState};

    let msg = state.messages.messages(session_id).iter().rev().find(|m| {
        m.tool.as_ref().is_some_and(|t| match &t.state {
            codeoid_protocol::ToolState::WaitingConfirmation {
                approval_id: aid, ..
            } => aid == approval_id,
            _ => false,
        })
    })?;
    let tool = msg.tool.as_ref()?;
    let name = tool.name.as_str();
    if name != "AskUserQuestion" && name != "ask_user_question" {
        return None;
    }
    let input = match &tool.state {
        codeoid_protocol::ToolState::WaitingConfirmation { input, .. } => input,
        _ => return None,
    };
    let questions_val = input.get("questions")?.as_array()?;
    let mut questions = Vec::with_capacity(questions_val.len());
    for q in questions_val {
        let question_text = q.get("question")?.as_str()?.to_string();
        let header = q.get("header").and_then(|h| h.as_str()).map(str::to_string);
        let multi_select = q
            .get("multiSelect")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let options_val = q.get("options")?.as_array()?;
        let options: Vec<AskOption> = options_val
            .iter()
            .filter_map(|o| {
                let label = o.get("label")?.as_str()?.to_string();
                let description = o
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(str::to_string);
                Some(AskOption { label, description })
            })
            .collect();
        if options.is_empty() {
            continue;
        }
        questions.push(AskUserQuestionState {
            question: question_text,
            header,
            multi_select,
            options,
            selected: Vec::new(),
        });
    }
    if questions.is_empty() {
        return None;
    }
    Some(AskUserQuestionModal {
        session_id: session_id.to_string(),
        approval_id: approval_id.to_string(),
        questions,
        focused_question: 0,
    })
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

/// Pure `session.create` frame builder — split from the async send path so
/// the wire shape (esp. the optional providerId) is unit-testable.
fn session_create_message(
    id: String,
    name: String,
    workdir: String,
    provider_id: Option<String>,
) -> ClientMessage {
    ClientMessage::SessionCreate {
        id,
        name,
        workdir,
        provider_id,
    }
}

/// Pure `session.set_provider` frame builder (see `session_create_message`).
fn set_provider_message(id: String, session_id: String, provider_id: String) -> ClientMessage {
    ClientMessage::SessionSetProvider {
        id,
        session_id,
        provider_id,
    }
}

/// Pure `session.fork` frame builder (see `session_create_message`). `name` is
/// left to the daemon default (parent name + " (fork)"); `provider_id` absent
/// keeps the parent's backend.
fn session_fork_message(
    id: String,
    session_id: String,
    provider_id: Option<String>,
) -> ClientMessage {
    ClientMessage::SessionFork {
        id,
        session_id,
        name: None,
        provider_id,
    }
}

/// Apply a `session.fork` response to the tab strip: add the returned fork and
/// focus it so its tab becomes active. Pure over [`AppState`] (no daemon I/O)
/// so the add-and-focus behaviour is unit-testable. `Err` carries a message
/// the caller surfaces if the daemon answered without a session payload.
fn apply_fork_response(
    state: &mut AppState,
    data: Option<serde_json::Value>,
) -> Result<(), String> {
    let fork: SessionInfo = data
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| "/fork: daemon returned no session".to_string())?;
    let fork_id = fork.id.clone();
    state.sessions.upsert(fork);
    state.sessions.focus_id(&fork_id);
    state.last_error = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use codeoid_protocol::{
        AuthOkMsg, IdentityType, MessageIdentity, ProviderCommand, SessionInfo,
        SessionUiRequestMsg, UiRequestMethod, UiResolvedReason,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::state::UiDialogModal;

    fn mk_state() -> AppState {
        let mut state = AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "spiffe://x".into(),
                name: Some("Me".into()),
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
            providers: None,
        });
        state.sessions.upsert(SessionInfo {
            id: "s1".into(),
            name: "demo".into(),
            workdir: "/tmp".into(),
            status: SessionStatus::Idle,
            created_by: "u".into(),
            created_at: "t".into(),
            attached_clients: 0,
            mode: None,
            turns_remaining: None,
            pinned_files: None,
            agent_uri: None,
            subagents: None,
            usage: None,
            rotation: None,
            queued_messages: None,
            model: None,
            fallback_model: None,
            provider_id: None,
        });
        state
    }

    /// App with state but no live connection — every network send is a
    /// clean early-return, so the reducer's state transitions can be
    /// exercised deterministically.
    fn mk_app() -> App {
        let mut app = App::new("ws://test".into(), "tok".into());
        app.state = Some(mk_state());
        app
    }

    fn mk_request(rid: &str, method: UiRequestMethod) -> SessionUiRequestMsg {
        SessionUiRequestMsg {
            session_id: "s1".into(),
            request_id: rid.into(),
            method,
            title: "T".into(),
            message: None,
            options: Some(vec!["a".into(), "b".into()]),
            placeholder: None,
            prefill: None,
            timeout_ms: None,
            timestamp: "t".into(),
        }
    }

    fn key(code: KeyCode) -> CtEvent {
        CtEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn ui_request_broadcast_opens_dialog_for_focused_session() {
        let mut app = mk_app();
        app.apply_daemon(DaemonMessage::SessionUiRequest(mk_request(
            "u1",
            UiRequestMethod::Select,
        )));
        let state = app.state.as_ref().unwrap();
        assert!(matches!(state.modal, Some(Modal::UiDialog(_))));
    }

    #[test]
    fn ui_resolved_broadcast_dismisses_and_reveals_next() {
        let mut app = mk_app();
        app.apply_daemon(DaemonMessage::SessionUiRequest(mk_request(
            "u1",
            UiRequestMethod::Select,
        )));
        app.apply_daemon(DaemonMessage::SessionUiRequest(mk_request(
            "u2",
            UiRequestMethod::Confirm,
        )));
        app.apply_daemon(DaemonMessage::SessionUiResolved {
            session_id: "s1".into(),
            request_id: "u1".into(),
            reason: UiResolvedReason::Answered,
            timestamp: "t".into(),
        });
        let state = app.state.as_ref().unwrap();
        match state.modal.as_ref() {
            Some(Modal::UiDialog(m)) => assert_eq!(m.request.request_id, "u2"),
            other => panic!("expected next dialog, got {other:?}"),
        }
    }

    #[test]
    fn commands_result_broadcast_lands_in_state() {
        let mut app = mk_app();
        app.apply_daemon(DaemonMessage::SessionCommandsResult {
            request_id: "r".into(),
            session_id: "s1".into(),
            provider_id: "pi".into(),
            commands: vec![ProviderCommand {
                name: "review".into(),
                description: None,
                source: None,
                argument_hint: None,
            }],
        });
        let state = app.state.as_ref().unwrap();
        assert!(state.is_provider_command("review"));
    }

    #[tokio::test]
    async fn dialog_actions_navigate_pick_and_settle_without_a_connection() {
        let mut app = mk_app();
        app.apply_daemon(DaemonMessage::SessionUiRequest(mk_request(
            "u1",
            UiRequestMethod::Select,
        )));

        app.apply_action(Action::UiDialogNext).await;
        app.apply_action(Action::UiDialogPrev).await;
        // y/n are confirm-only — must not settle a select dialog.
        app.apply_action(Action::UiDialogYes).await;
        assert!(app.state.as_ref().unwrap().modal.is_some());

        // Pick(1) selects option 1 and submits; with no connection the
        // response send is skipped but the local settle still runs.
        app.apply_action(Action::UiDialogPick(1)).await;
        assert!(app.state.as_ref().unwrap().modal.is_none());
        assert!(app.state.as_ref().unwrap().pending_ui_requests.is_empty());

        // Out-of-range pick on a fresh dialog is a no-op.
        app.apply_daemon(DaemonMessage::SessionUiRequest(mk_request(
            "u2",
            UiRequestMethod::Select,
        )));
        app.apply_action(Action::UiDialogPick(9)).await;
        assert!(app.state.as_ref().unwrap().modal.is_some());
        app.apply_action(Action::UiDialogCancel).await;
        assert!(app.state.as_ref().unwrap().modal.is_none());
    }

    #[tokio::test]
    async fn confirm_dialog_answers_via_yes_no() {
        let mut app = mk_app();
        app.apply_daemon(DaemonMessage::SessionUiRequest(mk_request(
            "u1",
            UiRequestMethod::Confirm,
        )));
        app.apply_action(Action::UiDialogNo).await;
        assert!(app.state.as_ref().unwrap().modal.is_none());
    }

    #[tokio::test]
    async fn text_dialog_buffers_keystrokes_and_paste() {
        let mut app = mk_app();
        let mut req = mk_request("u1", UiRequestMethod::Input);
        req.options = None;
        app.apply_daemon(DaemonMessage::SessionUiRequest(req));

        app.update(AppEvent::Terminal(key(KeyCode::Char('h'))))
            .await;
        app.update(AppEvent::Terminal(key(KeyCode::Char('i'))))
            .await;
        app.update(AppEvent::Terminal(key(KeyCode::Char('!'))))
            .await;
        app.update(AppEvent::Terminal(key(KeyCode::Backspace)))
            .await;
        app.update(AppEvent::Terminal(CtEvent::Paste(" there".into())))
            .await;

        match app.state.as_ref().unwrap().modal.as_ref() {
            Some(Modal::UiDialog(m)) => assert_eq!(m.buffer, "hi there"),
            other => panic!("expected text dialog, got {other:?}"),
        }
        // Enter submits (settles locally without a connection).
        app.update(AppEvent::Terminal(key(KeyCode::Enter))).await;
        assert!(app.state.as_ref().unwrap().modal.is_none());
    }

    #[tokio::test]
    async fn submit_ui_dialog_variants_cover_every_method() {
        let mut app = mk_app();

        // Editor: buffer submits.
        let mut editor = mk_request("u1", UiRequestMethod::Editor);
        editor.prefill = Some("draft".into());
        app.apply_daemon(DaemonMessage::SessionUiRequest(editor));
        app.submit_ui_dialog().await;
        assert!(app.state.as_ref().unwrap().modal.is_none());

        // Confirm: Enter submits as confirmed.
        app.apply_daemon(DaemonMessage::SessionUiRequest(mk_request(
            "u2",
            UiRequestMethod::Confirm,
        )));
        app.submit_ui_dialog().await;
        assert!(app.state.as_ref().unwrap().modal.is_none());

        // Select with empty options: submit is a no-op (nothing to choose).
        let mut empty = mk_request("u3", UiRequestMethod::Select);
        empty.options = Some(vec![]);
        app.apply_daemon(DaemonMessage::SessionUiRequest(empty));
        app.submit_ui_dialog().await;
        assert!(app.state.as_ref().unwrap().modal.is_some());
    }

    #[tokio::test]
    async fn maybe_fetch_commands_is_once_per_session_per_connection() {
        let mut app = mk_app();
        app.maybe_fetch_commands().await;
        assert!(app
            .state
            .as_ref()
            .unwrap()
            .commands_requested
            .contains("s1"));
        // Second call is a no-op (already recorded), covered by the guard.
        app.maybe_fetch_commands().await;
        app.on_focus_changed().await;
        assert_eq!(app.state.as_ref().unwrap().commands_requested.len(), 1);
    }

    #[tokio::test]
    async fn provider_command_verbs_fall_through_to_send() {
        let mut app = mk_app();
        app.state.as_mut().unwrap().provider_commands.insert(
            "s1".into(),
            vec![ProviderCommand {
                name: "review".into(),
                description: None,
                source: None,
                argument_hint: None,
            }],
        );

        // Catalogued verb: NOT a parse error — it falls through to the
        // send path (which, with no live handle, queues + reports offline;
        // that message proves the text reached SEND, not the parse error).
        app.state
            .as_mut()
            .unwrap()
            .prompt
            .insert_str("/review the diff");
        app.submit_prompt().await;
        assert!(app
            .state
            .as_ref()
            .unwrap()
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("offline — message queued")));
        assert_eq!(app.pending_sends.len(), 1, "queued for reconnect flush");

        // Unknown verb: still a visible parse error.
        app.state.as_mut().unwrap().prompt.insert_str("/nonsense");
        app.submit_prompt().await;
        assert!(app
            .state
            .as_ref()
            .unwrap()
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("nonsense")));
    }

    #[test]
    fn message_builders_produce_the_wire_shapes() {
        let create =
            session_create_message("r1".into(), "demo".into(), "/w".into(), Some("pi".into()));
        let json = serde_json::to_value(&create).unwrap();
        assert_eq!(json["type"], "session.create");
        assert_eq!(json["providerId"], "pi");

        // No provider → field entirely absent (daemon default applies).
        let plain = session_create_message("r2".into(), "demo".into(), "/w".into(), None);
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("providerId").is_none());

        let switch = set_provider_message("r3".into(), "s1".into(), "pi".into());
        let json = serde_json::to_value(&switch).unwrap();
        assert_eq!(json["type"], "session.set_provider");
        assert_eq!(json["sessionId"], "s1");
        assert_eq!(json["providerId"], "pi");

        // Fork onto another backend: type + session + backend, name defaulted
        // by the daemon (absent on the wire).
        let fork = session_fork_message("r4".into(), "s1".into(), Some("codex".into()));
        let json = serde_json::to_value(&fork).unwrap();
        assert_eq!(json["type"], "session.fork");
        assert_eq!(json["sessionId"], "s1");
        assert_eq!(json["providerId"], "codex");
        assert!(json.get("name").is_none());

        // Fork without a backend → providerId absent (parent's backend kept).
        let same = session_fork_message("r5".into(), "s1".into(), None);
        let json = serde_json::to_value(&same).unwrap();
        assert!(json.get("providerId").is_none());
    }

    #[test]
    fn apply_fork_response_adds_and_focuses_the_returned_fork() {
        let mut state = mk_state();
        // Parent "s1" is focused to start.
        assert_eq!(state.sessions.focused_id(), Some("s1"));

        // The daemon's fork payload: same shape as the parent, new id + name.
        let fork_info = SessionInfo {
            id: "s1-fork".into(),
            name: "demo (fork)".into(),
            ..state.sessions.items()[0].clone()
        };
        let data = Some(serde_json::to_value(&fork_info).unwrap());
        apply_fork_response(&mut state, data).expect("valid payload");

        // The fork is now a tab AND focused; the parent is untouched.
        assert_eq!(state.sessions.focused_id(), Some("s1-fork"));
        assert!(state.sessions.items().iter().any(|s| s.id == "s1"));
        assert!(state.sessions.items().iter().any(|s| s.id == "s1-fork"));
        assert!(state.last_error.is_none());
    }

    #[test]
    fn apply_fork_response_errors_without_a_session_payload() {
        let mut state = mk_state();
        // Daemon answered `ok` but with no data (or an unparseable body).
        assert!(apply_fork_response(&mut state, None)
            .unwrap_err()
            .contains("no session"));
        assert!(apply_fork_response(&mut state, Some(serde_json::json!({"junk": 1}))).is_err());
        // Focus stayed on the parent — nothing was added.
        assert_eq!(state.sessions.focused_id(), Some("s1"));
    }

    #[tokio::test]
    async fn fork_focused_requires_a_focused_session() {
        let mut app = App::new("ws://test".into(), "tok".into());
        app.state = Some(AppState::new(codeoid_protocol::AuthOkMsg {
            identity: codeoid_protocol::MessageIdentity {
                sub: "u".into(),
                name: None,
                kind: codeoid_protocol::IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
            providers: Some(vec!["claude".into(), "codex".into()]),
        }));
        app.fork_focused(None).await;
        assert!(app
            .state
            .as_ref()
            .unwrap()
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("no session focused")));
    }

    #[tokio::test]
    async fn set_provider_requires_a_focused_session() {
        let mut app = App::new("ws://test".into(), "tok".into());
        app.state = Some(AppState::new(codeoid_protocol::AuthOkMsg {
            identity: codeoid_protocol::MessageIdentity {
                sub: "u".into(),
                name: None,
                kind: codeoid_protocol::IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
            providers: Some(vec!["claude".into(), "pi".into()]),
        }));
        app.set_provider("pi".into()).await;
        assert!(app
            .state
            .as_ref()
            .unwrap()
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("no session focused")));
    }

    #[tokio::test]
    async fn provider_slash_commands_route_without_a_connection() {
        // With a focused session but no live handle, both paths return
        // cleanly (guard-path coverage; the wire shapes are unit-tested
        // above, and the daemon behavior in codeoid's own suite).
        let mut app = mk_app();
        app.state
            .as_mut()
            .unwrap()
            .prompt
            .insert_str("/provider pi");
        app.submit_prompt().await;
        app.state
            .as_mut()
            .unwrap()
            .prompt
            .insert_str("/new demo --provider pi");
        app.submit_prompt().await;
        // `/fork` and `/fork <backend>` route through dispatch and return at
        // the missing-handle guard without queueing anything.
        app.state.as_mut().unwrap().prompt.insert_str("/fork");
        app.submit_prompt().await;
        app.state.as_mut().unwrap().prompt.insert_str("/fork codex");
        app.submit_prompt().await;
        assert!(app.pending_sends.is_empty(), "no handle — nothing queued");
    }
}
