//! App-level state. Kept as a plain struct so it can be snapshotted for
//! diagnostics + tested without the renderer.

// Some public surfaces (unused Focus / Modal variants, helper methods on
// `SessionList`) are reserved for features that land in follow-up work —
// confirm-destroy modal, session switcher pane, explicit detach. Rather
// than delete + re-add them, we silence the warnings here. Prefer
// removing this `allow` once those features ship.
#![allow(dead_code)]

pub mod messages;
pub mod render_cache;
pub mod scrollback_build;
pub mod sessions;

use std::collections::{HashMap, HashSet};

use codeoid_protocol::{AuthOkMsg, SessionInfo};
use tui_textarea::TextArea;

use self::messages::MessageStore;
use self::render_cache::RenderCache;
use self::scrollback_build::ScrollbackBuild;
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
    /// Rows above the natural bottom of the transcript. 0 = sticky-to-
    /// latest (Bottom mode); positive = scrolled up (Anchored mode).
    /// While positive, the renderer auto-bumps this each frame as new
    /// content streams in at the bottom, so the user's view stays
    /// pinned to the content they were reading.
    pub scroll_offset: u16,
    /// Total rendered rows of the transcript on the previous frame.
    /// Used to detect "new content arrived at the bottom" and update
    /// `scroll_offset` + `unseen_below_rows` accordingly.
    pub last_total_rendered: usize,
    /// Rows that have arrived below the user's current viewport since
    /// they scrolled up. Surfaces as "↓ N new" in the worker row.
    /// Reset to 0 when they catch back up to the bottom.
    pub unseen_below_rows: usize,
    /// Inner height (post-borders) of the transcript viewport, captured
    /// each frame. Used to size PgUp/PgDn jumps so they move "almost a
    /// full page" with one row of overlap (the standard pager UX).
    pub last_viewport_rows: u16,
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
    /// Per-message styled-line cache. Lives at the app level so it
    /// survives across frames; invalidated by message version + width.
    /// See [`RenderCache`] for the keying rules.
    pub render_cache: RenderCache,
    /// Frame-to-frame cache of the *assembled* scrollback (every
    /// visible message's lines concatenated + the `total_rendered_rows`
    /// count for scroll math). Hits whenever the focused session, its
    /// epoch, and the viewport width are all unchanged — i.e. on every
    /// keystroke into the prompt and every idle frame. See
    /// [`ScrollbackBuild`] for the keying rules.
    pub scrollback_build: ScrollbackBuild,
    /// Global override for tool-output truncation. When true, every tool
    /// body renders fully (still capped at the verbose ceiling so a
    /// 10 000-line `find` doesn't melt the renderer). When false, the
    /// per-block `expanded_tool_message_ids` set is consulted instead so
    /// the user can pop individual blocks open. Toggled with `v`.
    pub verbose_tool_output: bool,
    /// Per-tool-block expand state for the "click to expand" equivalent
    /// in the TUI. Keys are the `tool_call` message ids whose bodies the
    /// user opted to render in full. Mirrors the web UI's
    /// click-to-expand behaviour, except keyed off `[`/`]` navigation +
    /// `Enter` to toggle, and only consulted when the global verbose
    /// override is off.
    pub expanded_tool_message_ids: HashSet<String>,
    /// Currently-selected tool block — the one `Enter` will expand /
    /// collapse. `]` and `[` walk through the tool_call messages of the
    /// focused session. `None` means "no explicit selection yet" and
    /// `Enter` falls back to the most recent tool_call message.
    pub selected_tool_message_id: Option<String>,
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
            last_total_rendered: 0,
            unseen_below_rows: 0,
            last_viewport_rows: 0,
            last_error: None,
            anim_tick: 0,
            activity_by_session: HashMap::new(),
            attached: HashSet::new(),
            connection: ConnectionState::Connected,
            render_cache: RenderCache::default(),
            scrollback_build: ScrollbackBuild::default(),
            verbose_tool_output: false,
            expanded_tool_message_ids: HashSet::new(),
            selected_tool_message_id: None,
        }
    }

    /// Tool_call message ids in the focused session, in transcript
    /// order. Used by `[` / `]` / `Enter` to walk and toggle individual
    /// tool blocks when the global verbose override is off.
    pub fn focused_tool_call_ids(&self) -> Vec<String> {
        let Some(sid) = self.sessions.focused_id() else {
            return Vec::new();
        };
        self.messages
            .messages(sid)
            .iter()
            .filter(|m| matches!(m.role, codeoid_protocol::MessageRole::ToolCall))
            .map(|m| m.message_id.clone())
            .collect()
    }

    /// Move the tool-block selection to the next or previous tool_call.
    /// Wraps at both ends. With no current selection, jumps to the last
    /// (newest) on next-press and the first on prev-press so the
    /// keybinding does something sensible from a cold start.
    pub fn cycle_tool_block_selection(&mut self, forward: bool) {
        let ids = self.focused_tool_call_ids();
        if ids.is_empty() {
            self.selected_tool_message_id = None;
            return;
        }
        let prev_selected = self.selected_tool_message_id.clone();
        let next = match self.selected_tool_message_id.as_deref() {
            Some(cur) => match ids.iter().position(|id| id == cur) {
                Some(idx) if forward => ids[(idx + 1) % ids.len()].clone(),
                Some(idx) => ids[(idx + ids.len() - 1) % ids.len()].clone(),
                None => {
                    if forward {
                        ids[0].clone()
                    } else {
                        ids[ids.len() - 1].clone()
                    }
                }
            },
            None => {
                if forward {
                    ids[ids.len() - 1].clone()
                } else {
                    ids[0].clone()
                }
            }
        };
        self.selected_tool_message_id = Some(next.clone());
        // Both the old and new selected blocks render with a different
        // header style, so invalidate per-message caches for them.
        if let Some(old) = prev_selected {
            self.render_cache.invalidate(&old);
        }
        self.render_cache.invalidate(&next);
        self.scrollback_build.clear();
    }

    /// Toggle the per-block expand state for the current selection.
    /// Falls back to the most recent tool_call if nothing is selected
    /// yet — the "expand the latest output" expectation when the user
    /// just presses `Enter` from the bottom of the transcript without
    /// having navigated.
    pub fn toggle_expand_selected_tool_block(&mut self) {
        let target = self
            .selected_tool_message_id
            .clone()
            .or_else(|| self.focused_tool_call_ids().last().cloned());
        let Some(id) = target else { return };
        if self.expanded_tool_message_ids.contains(&id) {
            self.expanded_tool_message_ids.remove(&id);
        } else {
            self.expanded_tool_message_ids.insert(id.clone());
        }
        self.selected_tool_message_id = Some(id.clone());
        self.render_cache.invalidate(&id);
        self.scrollback_build.clear();
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

    /// Scroll up by N rendered rows. Transitions Bottom → Anchored on
    /// the first row of upward movement.
    pub fn scroll_up(&mut self, by: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(by);
    }

    /// Scroll down by N rendered rows. Once the offset returns to 0 we
    /// drop the unseen-below counter — the user has caught up.
    pub fn scroll_down(&mut self, by: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(by);
        // The user has scrolled past some of the previously-unseen
        // content; clamp so the indicator never reports more "new
        // below" than actually remains below the viewport.
        self.unseen_below_rows = self.unseen_below_rows.min(self.scroll_offset as usize);
    }

    /// Jump back to the natural bottom (= sticky / following mode).
    /// Called on user "End" / "PgDn-to-zero" / submit / session switch.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.unseen_below_rows = 0;
    }

    /// Jump to the top of the transcript. Implementation: set offset to
    /// the maximum so the renderer's saturating math lands at row 0.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = u16::MAX;
    }

    /// Called by the renderer once it knows the post-wrap row count.
    /// Performs the "anchored" maintenance: when content streams in at
    /// the bottom while the user is scrolled up, bump `scroll_offset`
    /// by the same delta so the visible window stays pinned to the
    /// content the user was reading, and accumulate `unseen_below_rows`
    /// for the "↓ N new" indicator.
    pub fn note_total_rendered(&mut self, total: usize) {
        if self.scroll_offset > 0 && total > self.last_total_rendered {
            let delta = total - self.last_total_rendered;
            // u16 saturation is fine: at >65k rows below the fold, the
            // exact count stops being meaningful — the indicator just
            // shows "lots".
            self.scroll_offset = self
                .scroll_offset
                .saturating_add(delta.min(u16::MAX as usize) as u16);
            self.unseen_below_rows = self.unseen_below_rows.saturating_add(delta);
        }
        self.last_total_rendered = total;
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
    Capabilities(CapabilitiesModal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitiesTab {
    Agents,
    Skills,
    Mcp,
    Hooks,
}

#[derive(Debug, Clone)]
pub struct CapabilitiesModal {
    pub tab: CapabilitiesTab,
    pub loading: bool,
    pub error: Option<String>,
    pub workdir: Option<String>,
    pub agents: Vec<codeoid_protocol::ClaudeConfigAgent>,
    pub skills: Vec<codeoid_protocol::ClaudeConfigSkill>,
    pub mcp_servers: Vec<codeoid_protocol::ClaudeConfigMcpServer>,
    pub hooks: Vec<codeoid_protocol::ClaudeConfigHook>,
    /// Pending request id we're waiting on; used to drop stale results.
    pub pending_request_id: Option<String>,
    /// Selected row index within the active tab — for keyboard nav.
    pub selected: usize,
}

impl CapabilitiesModal {
    pub fn new(tab: CapabilitiesTab) -> Self {
        Self {
            tab,
            loading: true,
            error: None,
            workdir: None,
            agents: vec![],
            skills: vec![],
            mcp_servers: vec![],
            hooks: vec![],
            pending_request_id: None,
            selected: 0,
        }
    }
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

    // ============ Anchored scroll ============

    #[test]
    fn scroll_starts_at_bottom() {
        let state = mk_state();
        assert_eq!(state.scroll_offset, 0);
        assert_eq!(state.unseen_below_rows, 0);
    }

    #[test]
    fn new_content_at_bottom_doesnt_move_view_when_scrolled_up() {
        let mut state = mk_state();

        // Start at the bottom with 100 rendered rows.
        state.note_total_rendered(100);

        // User scrolls up 20 rows.
        state.scroll_up(20);
        assert_eq!(state.scroll_offset, 20);

        // 5 new rows arrive at the bottom. The user's view must STAY
        // pinned to the same content — scroll_offset should bump to 25
        // (still 20 rows above the previous anchor) and the indicator
        // counts 5 new arrivals.
        state.note_total_rendered(105);
        assert_eq!(state.scroll_offset, 25);
        assert_eq!(state.unseen_below_rows, 5);
    }

    #[test]
    fn new_content_at_bottom_doesnt_count_when_at_bottom() {
        let mut state = mk_state();
        state.note_total_rendered(50);
        // User is at the bottom (Bottom mode). New content shouldn't
        // be counted as "unseen" — they're already following.
        state.note_total_rendered(80);
        assert_eq!(state.scroll_offset, 0);
        assert_eq!(state.unseen_below_rows, 0);
    }

    #[test]
    fn scroll_down_clamps_unseen_to_remaining_below() {
        let mut state = mk_state();
        state.note_total_rendered(100);
        state.scroll_up(30);
        // 10 new rows → unseen=10, scroll=40.
        state.note_total_rendered(110);
        assert_eq!(state.unseen_below_rows, 10);

        // Scroll down 35 rows. Now only 5 rows below viewport — the
        // indicator can't honestly say there are 10 below.
        state.scroll_down(35);
        assert_eq!(state.scroll_offset, 5);
        assert_eq!(state.unseen_below_rows, 5);
    }

    #[test]
    fn scroll_to_bottom_clears_indicator() {
        let mut state = mk_state();
        // Real renderer always calls note_total before the user can
        // scroll (you need a frame to see what to scroll). Establish
        // that baseline first.
        state.note_total_rendered(200);
        state.scroll_up(50);
        state.note_total_rendered(220); // +20 unseen
        assert_eq!(state.unseen_below_rows, 20);

        state.scroll_to_bottom();
        assert_eq!(state.scroll_offset, 0);
        assert_eq!(state.unseen_below_rows, 0);
    }

    #[test]
    fn scroll_down_to_zero_clears_indicator() {
        let mut state = mk_state();
        state.note_total_rendered(100);
        state.scroll_up(10);
        state.note_total_rendered(115); // +15 unseen, scroll=25
        // PgDn: scroll all the way back.
        state.scroll_down(100); // saturates to 0
        assert_eq!(state.scroll_offset, 0);
        assert_eq!(state.unseen_below_rows, 0);
    }

    #[test]
    fn scroll_to_top_lands_at_zero_after_render() {
        // scroll_to_top sets offset to u16::MAX; the renderer's
        // saturating math turns that into row 0. Here we just verify
        // the state-side contract.
        let mut state = mk_state();
        state.scroll_to_top();
        assert_eq!(state.scroll_offset, u16::MAX);
    }

    #[test]
    fn shrinking_total_doesnt_underflow() {
        // Session switch / message prune can shrink total. note_total
        // must not panic.
        let mut state = mk_state();
        state.note_total_rendered(500);
        state.scroll_up(100);
        state.note_total_rendered(50); // huge shrink
        // No bump applied (delta is negative); last_total updates.
        assert_eq!(state.scroll_offset, 100);
        assert_eq!(state.unseen_below_rows, 0);
        assert_eq!(state.last_total_rendered, 50);
    }
}
