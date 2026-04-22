//! App-level state. Kept as a plain struct so it can be snapshotted for
//! diagnostics + tested without the renderer.

// Some public surfaces (unused Focus / Modal variants, helper methods on
// `SessionList`) are reserved for features that land in follow-up work —
// confirm-destroy modal, session switcher pane, explicit detach. Rather
// than delete + re-add them, we silence the warnings here. Prefer
// removing this `allow` once those features ship.
#![allow(dead_code)]

pub mod messages;
pub mod sessions;

use std::collections::{HashMap, HashSet};

use codeoid_protocol::{AuthOkMsg, SessionInfo};
use tui_textarea::TextArea;

use self::messages::MessageStore;
use self::sessions::SessionList;

/// Entire UI state. Every mutation goes through a single `apply_*` method
/// so tests can exercise the reducer without Ratatui or Tokio.
pub struct AppState {
    pub auth: AuthOkMsg,
    pub sessions: SessionList,
    pub messages: MessageStore,
    pub focus: Focus,
    pub modal: Option<Modal>,
    /// Prompt editor. `TextArea` handles cursor movement, word-level
    /// navigation, arrow keys, backspace/delete, multi-line, and
    /// rendering of the cursor glyph — everything a real editor needs.
    pub prompt: TextArea<'static>,
    pub scroll_offset: u16,
    pub last_error: Option<String>,
    /// Monotonically increasing tick counter driven by the 100 ms `Tick`
    /// event. Used by spinners and verb rotations to stay animated.
    pub anim_tick: u64,
    /// Per-session tick value at which we last saw streaming activity
    /// (assistant/thinking message, or any delta). Used as a fallback
    /// signal for the working indicator when `session.status` hasn't
    /// flipped yet. Keyed by session id so an active session A never
    /// "leaks" a Thinking spinner onto an idle session B when the user
    /// switches tabs.
    pub activity_by_session: HashMap<String, u64>,
    /// Session ids we've already sent `session.attach` for. The daemon
    /// only broadcasts messages (and echoes our own sends) to attached
    /// clients — without this set, `session.send` would succeed on the
    /// daemon but we'd never see any of the resulting traffic.
    pub attached: HashSet<String>,
    /// Connection health — surfaced as a pill in the status bar.
    pub connection: ConnectionState,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("focus", &self.focus)
            .field("connection", &self.connection)
            .field("sessions", &self.sessions.items().len())
            .finish_non_exhaustive()
    }
}

/// Observable connection state. The app transitions between these as the
/// daemon socket comes up, falls over, and recovers.
#[derive(Debug, Clone)]
pub enum ConnectionState {
    /// Live — normal operation.
    Connected,
    /// Socket dropped; app is waiting before reconnect.
    Reconnecting {
        attempt: u32,
        next_attempt_in_secs: u64,
    },
    /// Terminal error; no further reconnect attempts.
    Failed {
        reason: String,
    },
}

impl AppState {
    #[must_use]
    pub fn new(auth: AuthOkMsg) -> Self {
        let mut prompt = TextArea::default();
        prompt.set_cursor_line_style(ratatui::style::Style::default());
        prompt.set_placeholder_text("Message…  Enter sends · Shift+Enter newline · Esc blurs");
        Self {
            auth,
            sessions: SessionList::default(),
            messages: MessageStore::default(),
            focus: Focus::Prompt,
            modal: None,
            prompt,
            scroll_offset: 0,
            last_error: None,
            anim_tick: 0,
            activity_by_session: HashMap::new(),
            attached: HashSet::new(),
            connection: ConnectionState::Connected,
        }
    }

    /// Drain the prompt into a `String` and reset the editor. Returns
    /// `None` if the editor was empty (or whitespace-only).
    pub fn take_prompt(&mut self) -> Option<String> {
        let text = self.prompt.lines().join("\n");
        if text.trim().is_empty() {
            return None;
        }
        // TextArea doesn't have a clear() method; re-initialize.
        let mut fresh = TextArea::default();
        fresh.set_cursor_line_style(ratatui::style::Style::default());
        fresh.set_placeholder_text("Message…  Enter sends · Shift+Enter newline · Esc blurs");
        self.prompt = fresh;
        Some(text)
    }

    #[must_use]
    pub fn prompt_is_empty(&self) -> bool {
        self.prompt.lines().iter().all(|l| l.is_empty())
    }

    /// True when the prompt's first character is `/`, i.e. the user is
    /// typing a slash-command. The UI uses this to swap the prompt title,
    /// recolor the border, and show a filtered command palette in place
    /// of the worker row.
    #[must_use]
    pub fn is_command_mode(&self) -> bool {
        self.prompt
            .lines()
            .first()
            .is_some_and(|l| l.starts_with('/'))
    }

    /// Text after the leading `/`, across the first line only. `None` if
    /// not in command mode.
    #[must_use]
    pub fn command_query(&self) -> Option<&str> {
        self.prompt.lines().first()?.strip_prefix('/')
    }

    /// Record that we've sent `session.attach` for this session. Returns
    /// `true` if this is a new attachment (i.e. the caller should actually
    /// send the message to the daemon).
    pub fn note_attached(&mut self, session_id: &str) -> bool {
        self.attached.insert(session_id.to_string())
    }

    pub fn tick(&mut self) {
        self.anim_tick = self.anim_tick.wrapping_add(1);
    }

    /// Mark a streaming event for a specific session. Per-session so the
    /// "Thinking" indicator is never shown on an idle session just
    /// because a different one is busy.
    pub fn mark_activity(&mut self, session_id: &str) {
        self.activity_by_session
            .insert(session_id.to_string(), self.anim_tick);
    }

    /// Ticks since the last streaming event for `session_id`. `None` = no
    /// activity has ever been observed for that session.
    #[must_use]
    pub fn ticks_since_activity(&self, session_id: &str) -> Option<u64> {
        self.activity_by_session
            .get(session_id)
            .map(|t| self.anim_tick.wrapping_sub(*t))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeoid_protocol::{IdentityType, MessageIdentity};

    fn mk_state() -> AppState {
        AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "spiffe://x".into(),
                name: Some("Test".into()),
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
        })
    }

    #[test]
    fn take_prompt_returns_none_for_empty() {
        let mut state = mk_state();
        assert!(state.take_prompt().is_none());
    }

    #[test]
    fn take_prompt_returns_none_for_whitespace_only() {
        let mut state = mk_state();
        state.prompt.insert_str("   \n  ");
        assert!(state.take_prompt().is_none());
    }

    #[test]
    fn take_prompt_returns_content_and_clears_editor() {
        let mut state = mk_state();
        state.prompt.insert_str("hello world");
        let taken = state.take_prompt().expect("content");
        assert_eq!(taken, "hello world");
        assert!(state.prompt_is_empty());
    }

    #[test]
    fn take_prompt_preserves_multiline() {
        let mut state = mk_state();
        state.prompt.insert_str("line 1");
        state.prompt.insert_newline();
        state.prompt.insert_str("line 2");
        let taken = state.take_prompt().expect("content");
        assert_eq!(taken, "line 1\nline 2");
    }

    #[test]
    fn mark_activity_is_per_session() {
        let mut state = mk_state();
        state.anim_tick = 100;
        state.mark_activity("session-a");

        // Another session should NOT register as recently active — the
        // Thinking indicator must stay off when you're looking at an idle
        // session while a different one streams.
        assert!(state.ticks_since_activity("session-b").is_none());
        assert_eq!(state.ticks_since_activity("session-a"), Some(0));
    }

    #[test]
    fn ticks_since_activity_grows_with_anim_tick() {
        let mut state = mk_state();
        state.anim_tick = 100;
        state.mark_activity("s");
        state.anim_tick = 142;
        assert_eq!(state.ticks_since_activity("s"), Some(42));
    }

    #[test]
    fn note_attached_is_idempotent() {
        let mut state = mk_state();
        assert!(state.note_attached("s1"));
        assert!(!state.note_attached("s1"), "second call should return false");
        assert!(state.note_attached("s2"));
    }

    #[test]
    fn tick_wraps_safely() {
        let mut state = mk_state();
        state.anim_tick = u64::MAX;
        state.tick();
        // Previous tick value was wrapping_add(1) → 0. ticks_since should
        // still compute a sane value via wrapping_sub, not panic.
        state.mark_activity("s");
        state.anim_tick = state.anim_tick.wrapping_add(5);
        assert_eq!(state.ticks_since_activity("s"), Some(5));
    }
}
