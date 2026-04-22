//! The app reducer — owns `AppState`, drains the merged event stream, and
//! hands the renderer a snapshot on each tick.

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use codeoid_client::{ClientHandle, Connected, StreamEvent};
use codeoid_protocol::{ClientMessage, DaemonMessage};
use crossterm::event::{Event as CtEvent, EventStream, KeyEventKind};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::event::AppEvent;
use crate::keymap::{resolve, Action};
use crate::state::{AppState, Focus, Modal};
use crate::ui;

const TICK: Duration = Duration::from_millis(100);

pub struct App {
    state: AppState,
    handle: ClientHandle,
    daemon_events: mpsc::Receiver<StreamEvent>,
    quit_requested: bool,
}

impl App {
    #[must_use]
    pub fn new(connected: Connected) -> Self {
        let mut state = AppState::new(connected.auth);
        if let Some(drift) = connected.version_warning {
            state.modal = Some(Modal::ProtocolDrift {
                client: drift.client,
                daemon: drift.daemon,
            });
        }
        Self {
            state,
            handle: connected.handle,
            daemon_events: connected.events,
            quit_requested: false,
        }
    }

    pub async fn run(
        mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        // Kick off an initial session list so the tabs aren't empty.
        let id = ClientHandle::next_request_id();
        let _ = self
            .handle
            .send(ClientMessage::SessionList { id })
            .await;

        let mut term_events = EventStream::new();
        let mut ticker = interval(TICK);
        let started = Instant::now();

        loop {
            terminal.draw(|f| ui::render(f, &self.state))?;

            let event = tokio::select! {
                Some(ev) = term_events.next() => match ev {
                    Ok(e) => AppEvent::Terminal(e),
                    Err(e) => {
                        warn!(error = %e, "terminal event stream error");
                        continue;
                    }
                },
                Some(ev) = self.daemon_events.recv() => AppEvent::Net(ev),
                _ = ticker.tick() => AppEvent::Tick,
            };

            self.update(event).await;

            if self.quit_requested {
                debug!(uptime_ms = ?started.elapsed().as_millis(), "quit requested");
                self.handle.shutdown().await;
                return Ok(());
            }
        }
    }

    async fn update(&mut self, event: AppEvent) {
        match event {
            AppEvent::Terminal(CtEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                let prompt_focused = self.state.focus == Focus::Prompt;
                if let Some(action) = resolve(key, prompt_focused) {
                    self.apply_action(action).await;
                } else if prompt_focused {
                    if let crossterm::event::KeyCode::Char(c) = key.code {
                        self.state.prompt_buffer.push(c);
                    } else if matches!(key.code, crossterm::event::KeyCode::Backspace) {
                        self.state.prompt_buffer.pop();
                    }
                }
            }
            AppEvent::Terminal(CtEvent::Resize(_, _)) => {
                // Ratatui handles resize on the next draw; nothing to do.
            }
            AppEvent::Terminal(_) => {}
            AppEvent::Net(StreamEvent::Daemon(msg)) => self.apply_daemon(msg),
            AppEvent::Net(StreamEvent::Closed) => {
                self.state.record_error("daemon closed the connection");
                self.quit_requested = true;
            }
            AppEvent::Net(StreamEvent::Errored(err)) => {
                self.state.record_error(format!("daemon error: {err}"));
                self.quit_requested = true;
            }
            AppEvent::Tick | AppEvent::Quit => {}
        }
    }

    async fn apply_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit_requested = true,
            Action::FocusPrompt => self.state.focus = Focus::Prompt,
            Action::BlurPrompt => self.state.focus = Focus::Scrollback,
            Action::SubmitPrompt => self.submit_prompt().await,
            Action::NewlineInPrompt => self.state.prompt_buffer.push('\n'),
            Action::NextSession => self.state.sessions.focus_next(),
            Action::PrevSession => self.state.sessions.focus_prev(),
            Action::Interrupt => self.interrupt().await,
            Action::Approve => self.approve(true).await,
            Action::Deny => self.approve(false).await,
            Action::CycleMode => {
                // TODO: implement via session.set_mode
                info!("cycle mode: not yet implemented");
            }
            Action::ToggleHelp => {
                self.state.modal = match self.state.modal {
                    Some(Modal::Help) => None,
                    _ => Some(Modal::Help),
                };
            }
            Action::ScrollUp => self.state.scroll_offset = self.state.scroll_offset.saturating_add(1),
            Action::ScrollDown => {
                self.state.scroll_offset = self.state.scroll_offset.saturating_sub(1);
            }
            Action::PageUp => self.state.scroll_offset = self.state.scroll_offset.saturating_add(10),
            Action::PageDown => {
                self.state.scroll_offset = self.state.scroll_offset.saturating_sub(10);
            }
        }
    }

    fn apply_daemon(&mut self, msg: DaemonMessage) {
        match msg {
            DaemonMessage::SessionListResult { sessions, .. } => {
                self.state.set_sessions(sessions);
            }
            DaemonMessage::SessionInfoUpdate { session, .. } => {
                self.state.merge_session(session);
            }
            DaemonMessage::SessionStatusChange {
                session_id, status, ..
            } => {
                if let Some(s) = self
                    .state
                    .sessions
                    .items()
                    .iter()
                    .find(|s| s.id == session_id)
                    .cloned()
                {
                    let mut updated = s;
                    updated.status = status;
                    self.state.merge_session(updated);
                }
            }
            DaemonMessage::SessionMessage(m) => self.state.messages.apply_message(m),
            DaemonMessage::SessionMessageDelta(d) => self.state.messages.apply_delta(d),
            DaemonMessage::ScrollbackReplay {
                session_id,
                messages,
            } => self.state.messages.replace_scrollback(session_id, messages),
            DaemonMessage::SessionSearchResult { .. }
            | DaemonMessage::AuthOk(_)
            | DaemonMessage::ResponseOk { .. }
            | DaemonMessage::ResponseError { .. } => {
                // Solicited; handled by the request registry. If we got here
                // it's because no one was waiting — safe to ignore.
            }
            DaemonMessage::Unknown => {
                warn!("received unknown daemon message; forward-compat drop");
            }
        }
    }

    async fn submit_prompt(&mut self) {
        let Some(session) = self.state.sessions.focused().cloned() else {
            self.state.record_error("no session focused");
            return;
        };
        let text = std::mem::take(&mut self.state.prompt_buffer);
        if text.trim().is_empty() {
            return;
        }
        let id = ClientHandle::next_request_id();
        let msg = ClientMessage::SessionSend {
            id,
            session_id: session.id,
            text,
            attachments: None,
            priority: None,
        };
        if let Err(e) = self.handle.send(msg).await {
            self.state.record_error(format!("send failed: {e}"));
        }
    }

    async fn interrupt(&mut self) {
        let Some(session) = self.state.sessions.focused() else { return };
        let id = ClientHandle::next_request_id();
        let msg = ClientMessage::SessionInterrupt {
            id,
            session_id: session.id.clone(),
        };
        if let Err(e) = self.handle.send(msg).await {
            self.state.record_error(format!("interrupt failed: {e}"));
        }
    }

    async fn approve(&mut self, approved: bool) {
        // MVP: pick the most recent waiting_confirmation approval across the
        // focused session and respond to it.
        let Some(session) = self.state.sessions.focused() else { return };
        let Some(approval_id) = find_latest_approval(&self.state, &session.id) else {
            return;
        };
        let id = ClientHandle::next_request_id();
        let msg = ClientMessage::SessionApprove {
            id,
            session_id: session.id.clone(),
            approval_id,
            approved,
        };
        if let Err(e) = self.handle.send(msg).await {
            self.state.record_error(format!("approval failed: {e}"));
        }
    }
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
