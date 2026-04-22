//! App-level state. Kept as a plain struct so it can be snapshotted for
//! diagnostics + tested without the renderer.

pub mod messages;
pub mod sessions;

use codeoid_protocol::{AuthOkMsg, SessionInfo};

use self::messages::MessageStore;
use self::sessions::SessionList;

/// Entire UI state. Every mutation goes through a single `apply_*` method
/// so tests can exercise the reducer without Ratatui or Tokio.
#[derive(Debug)]
pub struct AppState {
    pub auth: AuthOkMsg,
    pub sessions: SessionList,
    pub messages: MessageStore,
    pub focus: Focus,
    pub modal: Option<Modal>,
    pub prompt_buffer: String,
    pub scroll_offset: u16,
    pub last_error: Option<String>,
}

impl AppState {
    #[must_use]
    pub fn new(auth: AuthOkMsg) -> Self {
        Self {
            auth,
            sessions: SessionList::default(),
            messages: MessageStore::default(),
            focus: Focus::Scrollback,
            modal: None,
            prompt_buffer: String::new(),
            scroll_offset: 0,
            last_error: None,
        }
    }

    pub fn set_sessions(&mut self, sessions: Vec<SessionInfo>) {
        self.sessions.replace(sessions);
    }

    pub fn merge_session(&mut self, session: SessionInfo) {
        self.sessions.upsert(session);
    }

    pub fn record_error(&mut self, err: impl Into<String>) {
        self.last_error = Some(err.into());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Scrollback,
    Prompt,
    SessionSwitcher,
}

#[derive(Debug, Clone)]
pub enum Modal {
    Help,
    ConfirmDestroy { session_id: String, name: String },
    ProtocolDrift { client: u32, daemon: Option<u32> },
}
