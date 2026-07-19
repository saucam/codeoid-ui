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

use codeoid_protocol::{
    AuthOkMsg, ModelInfo, ProviderCommand, SessionInfo, SessionUiRequestMsg, UiRequestMethod,
};
use tui_textarea::TextArea;

use self::messages::MessageStore;
use self::render_cache::RenderCache;
use self::scrollback_build::ScrollbackBuildCache;
use self::sessions::SessionList;

/// Entire UI state. Every mutation goes through a single `apply_*` method
/// so tests can exercise the reducer without Ratatui or Tokio.
pub struct AppState {
    pub auth: AuthOkMsg,
    /// Selectable model catalog from the backend (`models.list`). Empty
    /// until the result lands; used to validate `/model` and to map a
    /// model value to its human display name.
    pub models: Vec<ModelInfo>,
    /// Which backend `models` reflects — so a `/provider` switch refetches
    /// and a stale result (fast switch) can be dropped. None = unfetched.
    pub models_provider: Option<String>,
    /// True when `models` came from the live backend (vs a built-in fallback).
    pub models_live: bool,
    pub sessions: SessionList,
    pub messages: MessageStore,
    pub focus: Focus,
    pub modal: Option<Modal>,
    /// Pending provider dialogs (`session.ui_request`) per session, oldest
    /// first. Mirrors the daemon: add on request (attach re-delivery makes
    /// duplicates normal — dedupe by `request_id`), drop on
    /// `session.ui_resolved`. The focused session's head request opens the
    /// [`Modal::UiDialog`] when no other modal is up.
    pub pending_ui_requests: HashMap<String, Vec<SessionUiRequestMsg>>,
    /// Provider-defined slash commands per session (`session.commands`).
    /// Merged into the `/` palette and consulted as a parse fallback so
    /// `/review …` reaches the provider as plain prompt text.
    pub provider_commands: HashMap<String, Vec<ProviderCommand>>,
    /// Sessions whose command catalog has been requested this connection —
    /// fetch-once bookkeeping (reset on reconnect).
    pub commands_requested: HashSet<String>,
    /// Prompt editor. `TextArea` handles cursor movement, word-level
    /// navigation, arrow keys, backspace/delete, multi-line, and
    /// rendering of the cursor glyph — everything a real editor needs.
    pub prompt: TextArea<'static>,
    /// Rows above the natural bottom of the transcript. 0 = sticky-to-
    /// latest (Bottom mode); positive = scrolled up (Anchored mode).
    /// While positive, the renderer auto-bumps this each frame as new
    /// content streams in at the bottom, so the user's view stays
    /// pinned to the content they were reading. `usize` on purpose: a
    /// u16 caps at 65 535 wrapped rows, which made the top of large
    /// sessions unreachable. Only the intra-viewport remainder is ever
    /// handed to ratatui's u16 `Paragraph::scroll`.
    pub scroll_offset: usize,
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
    /// Sessions with history older than what's buffered (tail-first attach
    /// `hasMore`, then per page-result). Scrolling past the top fetches it.
    pub has_older_history: HashSet<String>,
    /// Session with a `scrollback.page` fetch in flight (anim_tick when it
    /// started, for a stale-fetch timeout) — one at a time is plenty.
    pub paging_in_flight: Option<(String, u64)>,
    /// One-shot: the next `note_total_rendered` growth is a top-PREPEND
    /// (older history), not bottom growth — skip the anchored-offset bump.
    /// The offset is measured from the BOTTOM, so keeping it unchanged is
    /// exactly what pins the viewport across a prepend.
    pub suppress_growth_anchor_once: bool,
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
    /// count for scroll math), kept per session in a small LRU so
    /// switching focus back to a recent session is a hit. Hits whenever
    /// the focused session's epoch and the viewport width are unchanged
    /// — i.e. on every keystroke into the prompt and every idle frame.
    /// See [`ScrollbackBuildCache`] for the keying rules.
    pub scrollback_build: ScrollbackBuildCache,
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
    Failed { reason: String },
}

impl AppState {
    #[must_use]
    pub fn new(auth: AuthOkMsg) -> Self {
        let mut prompt = TextArea::default();
        prompt.set_cursor_line_style(ratatui::style::Style::default());
        prompt.set_placeholder_text("Message…  Enter sends · Shift+Enter newline · Esc blurs");
        Self {
            auth,
            models: Vec::new(),
            models_provider: None,
            models_live: false,
            sessions: SessionList::default(),
            messages: MessageStore::default(),
            focus: Focus::Prompt,
            modal: None,
            pending_ui_requests: HashMap::new(),
            provider_commands: HashMap::new(),
            commands_requested: HashSet::new(),
            prompt,
            scroll_offset: 0,
            last_total_rendered: 0,
            unseen_below_rows: 0,
            last_viewport_rows: 0,
            last_error: None,
            anim_tick: 0,
            activity_by_session: HashMap::new(),
            has_older_history: HashSet::new(),
            paging_in_flight: None,
            suppress_growth_anchor_once: false,
            attached: HashSet::new(),
            connection: ConnectionState::Connected,
            render_cache: RenderCache::default(),
            scrollback_build: ScrollbackBuildCache::default(),
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
        // `ids` was non-empty, so a focused session id exists. Selection
        // is focused-session-local, so only that session's assembled
        // build is stale — evicting just it (instead of clearing the
        // whole LRU) keeps other sessions' A→B→A build hits alive and
        // preserves the "render-cache sessions ⊆ build-LRU sessions"
        // memory bound.
        if let Some(sid) = self.sessions.focused_id().map(ToString::to_string) {
            if let Some(old) = prev_selected {
                self.render_cache.invalidate(&sid, &old);
            }
            self.render_cache.invalidate(&sid, &next);
            self.scrollback_build.evict_session(&sid);
        }
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
        // Expand state is focused-session-local (the target id came
        // from the focused session's tool calls) — evict only that
        // session's build, same as cycle_tool_block_selection.
        if let Some(sid) = self.sessions.focused_id().map(ToString::to_string) {
            self.render_cache.invalidate(&sid, &id);
            self.scrollback_build.evict_session(&sid);
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
        self.prune_dead_session_state();
    }

    /// Drop per-session client state for sessions the daemon no longer
    /// reports (destroyed in another client, expired, …). Without this a
    /// long-lived TUI attached to a busy daemon leaks a transcript's worth
    /// of memory per dead session. The render caches are LRU-bounded and
    /// need no help here.
    fn prune_dead_session_state(&mut self) {
        let live: HashSet<String> = self.sessions.items().iter().map(|s| s.id.clone()).collect();
        self.messages.retain_sessions(&live);
        self.activity_by_session.retain(|sid, _| live.contains(sid));
        self.attached.retain(|sid| live.contains(sid));
        self.has_older_history.retain(|sid| live.contains(sid));
        self.pending_ui_requests.retain(|sid, _| live.contains(sid));
        // Release the single global paging lock if its session is gone —
        // otherwise a fetch in flight when a session is destroyed blocks
        // every OTHER session from paging until the 10s tick timeout.
        if self
            .paging_in_flight
            .as_ref()
            .is_some_and(|(sid, _)| !live.contains(sid))
        {
            self.paging_in_flight = None;
        }
    }

    pub fn merge_session(&mut self, session: SessionInfo) {
        self.sessions.upsert(session);
    }

    pub fn record_error(&mut self, err: impl Into<String>) {
        self.last_error = Some(err.into());
    }

    /// Human label for a model value, looked up in the fetched catalog;
    /// falls back to the raw value when the catalog is empty or unknown.
    #[must_use]
    pub fn model_display(&self, value: &str) -> String {
        self.models
            .iter()
            .find(|m| m.value == value)
            .map(|m| m.display_name.clone())
            .unwrap_or_else(|| value.to_string())
    }

    /// Scroll up by N rendered rows. Transitions Bottom → Anchored on
    /// the first row of upward movement.
    pub fn scroll_up(&mut self, by: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(by);
    }

    /// Scroll down by N rendered rows. Once the offset returns to 0 we
    /// drop the unseen-below counter — the user has caught up.
    pub fn scroll_down(&mut self, by: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(by);
        // The user has scrolled past some of the previously-unseen
        // content; clamp so the indicator never reports more "new
        // below" than actually remains below the viewport.
        self.unseen_below_rows = self.unseen_below_rows.min(self.scroll_offset);
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
        self.scroll_offset = usize::MAX;
    }

    /// Called by the renderer once it knows the post-wrap row count.
    /// Performs the "anchored" maintenance: when content streams in at
    /// the bottom while the user is scrolled up, bump `scroll_offset`
    /// by the same delta so the visible window stays pinned to the
    /// content the user was reading, and accumulate `unseen_below_rows`
    /// for the "↓ N new" indicator.
    pub fn note_total_rendered(&mut self, total: usize) {
        if self.scroll_offset > 0 && total > self.last_total_rendered {
            if self.suppress_growth_anchor_once {
                // Growth came from a top-PREPEND (older-history page). The
                // bottom-anchored offset already pins the viewport; bumping
                // it would shift the view up by the prepended rows.
                self.suppress_growth_anchor_once = false;
            } else {
                let delta = total - self.last_total_rendered;
                self.scroll_offset = self.scroll_offset.saturating_add(delta);
                self.unseen_below_rows = self.unseen_below_rows.saturating_add(delta);
            }
        }
        self.last_total_rendered = total;
    }

    // ── Provider dialogs (session.ui_request) ─────────────────────────────

    /// Record a pending dialog. Attach re-delivery makes duplicates normal —
    /// upsert by `request_id`.
    pub fn add_ui_request(&mut self, req: SessionUiRequestMsg) {
        let list = self
            .pending_ui_requests
            .entry(req.session_id.clone())
            .or_default();
        if !list.iter().any(|r| r.request_id == req.request_id) {
            list.push(req);
        }
    }

    /// Drop a settled dialog (answered anywhere, timed out, interrupted).
    /// Also closes the open modal if it was showing this request.
    pub fn remove_ui_request(&mut self, session_id: &str, request_id: &str) {
        if let Some(list) = self.pending_ui_requests.get_mut(session_id) {
            list.retain(|r| r.request_id != request_id);
            if list.is_empty() {
                self.pending_ui_requests.remove(session_id);
            }
        }
        if let Some(Modal::UiDialog(m)) = self.modal.as_ref() {
            if m.request.session_id == session_id && m.request.request_id == request_id {
                self.modal = None;
            }
        }
    }

    /// Open the focused session's oldest pending dialog — only when no other
    /// modal is up (never steal an approval form or a confirm-destroy).
    pub fn maybe_open_ui_dialog(&mut self) {
        if self.modal.is_some() {
            return;
        }
        let Some(focused_id) = self.sessions.focused().map(|s| s.id.clone()) else {
            return;
        };
        let Some(req) = self
            .pending_ui_requests
            .get(&focused_id)
            .and_then(|list| list.first())
            .cloned()
        else {
            return;
        };
        self.modal = Some(Modal::UiDialog(UiDialogModal::new(req)));
    }

    /// Provider commands for the focused session ([] until fetched).
    #[must_use]
    pub fn focused_provider_commands(&self) -> &[ProviderCommand] {
        self.sessions
            .focused()
            .and_then(|s| self.provider_commands.get(&s.id))
            .map_or(&[], Vec::as_slice)
    }

    /// Case-insensitive membership test — the parse fallback that lets
    /// `/review …` reach the provider as plain prompt text.
    #[must_use]
    pub fn is_provider_command(&self, name: &str) -> bool {
        self.focused_provider_commands()
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(name))
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
    AskUserQuestion(AskUserQuestionModal),
    UiDialog(UiDialogModal),
    Settings(SettingsModal),
}

/// Modal for a provider-initiated dialog (`session.ui_request`). Unlike
/// AskUserQuestion (which rides the tool-approval flow), these settle via
/// `session.ui_response`. One dialog at a time — resolving the head request
/// opens the next pending one for the focused session.
#[derive(Debug, Clone)]
pub struct UiDialogModal {
    pub request: SessionUiRequestMsg,
    /// Cursor into `request.options` for `method: select`.
    pub selected: usize,
    /// Text buffer for `method: input` / `editor` (seeded from `prefill`).
    pub buffer: String,
}

impl UiDialogModal {
    #[must_use]
    pub fn new(request: SessionUiRequestMsg) -> Self {
        let buffer = request.prefill.clone().unwrap_or_default();
        Self {
            request,
            selected: 0,
            buffer,
        }
    }

    /// True for methods whose primary interaction is typing — unresolved
    /// keystrokes feed [`Self::buffer`] instead of being absorbed.
    #[must_use]
    pub fn is_text_entry(&self) -> bool {
        matches!(
            self.request.method,
            UiRequestMethod::Input | UiRequestMethod::Editor
        )
    }

    pub fn next_option(&mut self) {
        let len = self.request.options.as_ref().map_or(0, Vec::len);
        if len > 0 {
            self.selected = (self.selected + 1) % len;
        }
    }

    pub fn prev_option(&mut self) {
        let len = self.request.options.as_ref().map_or(0, Vec::len);
        if len > 0 {
            self.selected = (self.selected + len - 1) % len;
        }
    }

    /// The currently selected option label (`method: select`).
    #[must_use]
    pub fn selected_option(&self) -> Option<&str> {
        self.request
            .options
            .as_ref()
            .and_then(|opts| opts.get(self.selected))
            .map(String::as_str)
    }
}

/// Per-question selection state for the AskUserQuestion form. For
/// single-select questions, the inner `Vec<usize>` will hold at most
/// one option index. For multi-select, it can hold many.
#[derive(Debug, Clone)]
pub struct AskUserQuestionState {
    pub question: String,
    pub header: Option<String>,
    pub multi_select: bool,
    pub options: Vec<AskOption>,
    pub selected: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct AskOption {
    pub label: String,
    pub description: Option<String>,
}

/// Modal opened automatically when the focused session's latest tool_call
/// is `AskUserQuestion` and the user presses Approve. The modal collects
/// answers across all questions, then sends a `session.approve` with an
/// `updated_input` patch carrying the answers map. Cancelling sends the
/// usual deny.
#[derive(Debug, Clone)]
pub struct AskUserQuestionModal {
    pub session_id: String,
    pub approval_id: String,
    pub questions: Vec<AskUserQuestionState>,
    /// Index into `questions` — which question the keyboard is acting on.
    pub focused_question: usize,
}

impl AskUserQuestionModal {
    /// All questions have at least one selected option. Submit is only
    /// enabled in this state.
    #[must_use]
    pub fn all_answered(&self) -> bool {
        self.questions.iter().all(|q| !q.selected.is_empty())
    }

    /// Build the daemon-bound `answers` map: `{question -> "label, …"}`.
    /// Multi-select picks are joined with ", " — matches the SDK's
    /// expected single-string-per-question shape.
    #[must_use]
    pub fn build_answers(&self) -> std::collections::HashMap<String, String> {
        let mut out = std::collections::HashMap::new();
        for q in &self.questions {
            let labels: Vec<String> = q
                .selected
                .iter()
                .filter_map(|&i| q.options.get(i).map(|o| o.label.clone()))
                .collect();
            out.insert(q.question.clone(), labels.join(", "));
        }
        out
    }

    /// Toggle option N for the focused question. For single-select,
    /// flips the selection to that option (replacing prior). For
    /// multi-select, adds/removes from the set.
    pub fn toggle_option(&mut self, idx: usize) {
        let Some(q) = self.questions.get_mut(self.focused_question) else {
            return;
        };
        if idx >= q.options.len() {
            return;
        }
        if q.multi_select {
            if let Some(pos) = q.selected.iter().position(|&i| i == idx) {
                q.selected.remove(pos);
            } else {
                q.selected.push(idx);
            }
        } else {
            q.selected = vec![idx];
        }
    }

    pub fn next_question(&mut self) {
        if self.questions.is_empty() {
            return;
        }
        self.focused_question = (self.focused_question + 1) % self.questions.len();
    }

    pub fn prev_question(&mut self) {
        if self.questions.is_empty() {
            return;
        }
        self.focused_question =
            (self.focused_question + self.questions.len() - 1) % self.questions.len();
    }
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

/// The comprehensive settings screen. Renders whatever manifest the daemon
/// serves over `settings.schema` (daemon-wide — no session needed): a tab
/// rail (one per manifest tab, incl. a tab per backend) + grouped fields with
/// a per-`kind` control. Edits are staged in `dirty` and committed as one
/// `settings.set` batch. Text/number/secret fields drop into `editing` +
/// `buffer` (a hand-rolled input, like the UiDialog text path); booleans and
/// enums toggle/cycle in place.
#[derive(Debug, Clone)]
pub struct SettingsModal {
    pub loading: bool,
    pub error: Option<String>,
    pub manifest: Option<codeoid_protocol::SettingsManifest>,
    pub snapshot: Option<codeoid_protocol::SettingsSnapshot>,
    /// Active tab index into `manifest.tabs`.
    pub tab: usize,
    /// Selected field index within the active tab's visible fields.
    pub selected: usize,
    pub show_advanced: bool,
    /// Staged edits, keyed by field key. `Value::Null` = clear / unset.
    pub dirty: std::collections::HashMap<String, serde_json::Value>,
    /// Field key currently in text-edit mode (`None` = navigating).
    pub editing: Option<String>,
    /// Text buffer backing the active edit.
    pub buffer: String,
    /// Last save outcome, shown in the footer.
    pub status: Option<String>,
    pub restart_required: bool,
    pub pending_schema_id: Option<String>,
    pub pending_get_id: Option<String>,
    pub pending_set_id: Option<String>,
}

impl Default for SettingsModal {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsModal {
    #[must_use]
    pub fn new() -> Self {
        Self {
            loading: true,
            error: None,
            manifest: None,
            snapshot: None,
            tab: 0,
            selected: 0,
            show_advanced: false,
            dirty: std::collections::HashMap::new(),
            editing: None,
            buffer: String::new(),
            status: None,
            restart_required: false,
            pending_schema_id: None,
            pending_get_id: None,
            pending_set_id: None,
        }
    }

    /// Fields visible on the active tab (advanced hidden unless toggled).
    #[must_use]
    pub fn tab_fields(&self) -> Vec<&codeoid_protocol::SettingField> {
        let Some(m) = self.manifest.as_ref() else {
            return Vec::new();
        };
        let Some(t) = m.tabs.get(self.tab) else {
            return Vec::new();
        };
        t.groups
            .iter()
            .flat_map(|g| g.fields.iter())
            .filter(|f| self.show_advanced || !f.advanced)
            .collect()
    }

    /// True when the daemon reported registry MCP servers — surfaced as a
    /// synthetic read-only tab appended after the manifest tabs.
    #[must_use]
    pub fn has_mcp_tab(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| !s.mcp_servers.is_empty())
    }

    /// True when the synthetic MCP servers tab is the active tab.
    #[must_use]
    pub fn on_mcp_tab(&self) -> bool {
        self.has_mcp_tab()
            && self
                .manifest
                .as_ref()
                .is_some_and(|m| self.tab == m.tabs.len())
    }

    #[must_use]
    pub fn tab_count(&self) -> usize {
        self.manifest.as_ref().map_or(0, |m| m.tabs.len()) + usize::from(self.has_mcp_tab())
    }

    /// Clamp the active tab into range after the tab set changes — e.g. the
    /// synthetic MCP tab appears/disappears when a fresh snapshot arrives. Keeps
    /// a stale out-of-range index (parked on the MCP tab, then servers vanish)
    /// from leaving a blank body until the next tab keypress.
    pub fn clamp_tab(&mut self) {
        let n = self.tab_count();
        if self.tab >= n {
            self.tab = n.saturating_sub(1);
            self.selected = 0;
        }
    }

    /// The field the cursor is on (cloned so callers avoid borrow conflicts).
    #[must_use]
    pub fn selected_field(&self) -> Option<codeoid_protocol::SettingField> {
        self.tab_fields().get(self.selected).map(|f| (*f).clone())
    }

    pub fn next_field(&mut self) {
        let n = self.tab_fields().len();
        if n > 0 {
            self.selected = (self.selected + 1) % n;
        }
    }

    pub fn prev_field(&mut self) {
        let n = self.tab_fields().len();
        if n > 0 {
            self.selected = (self.selected + n - 1) % n;
        }
    }

    pub fn next_tab(&mut self) {
        let n = self.tab_count();
        if n > 0 {
            self.tab = (self.tab + 1) % n;
            self.selected = 0;
        }
    }

    pub fn prev_tab(&mut self) {
        let n = self.tab_count();
        if n > 0 {
            self.tab = (self.tab + n - 1) % n;
            self.selected = 0;
        }
    }

    /// Staged-or-current value of a non-secret field key.
    #[must_use]
    pub fn effective(&self, key: &str) -> serde_json::Value {
        if let Some(v) = self.dirty.get(key) {
            return v.clone();
        }
        if let Some(snap) = self.snapshot.as_ref() {
            if let Some(st) = snap.values.get(key) {
                return st.value.clone();
            }
        }
        serde_json::Value::Null
    }

    /// The `kind` of a field anywhere in the manifest (used when committing
    /// an edit — the edited field is always on the active tab, but searching
    /// the whole manifest keeps this robust).
    #[must_use]
    pub fn field_kind(&self, key: &str) -> Option<String> {
        self.manifest
            .as_ref()?
            .tabs
            .iter()
            .flat_map(|t| t.groups.iter())
            .flat_map(|g| g.fields.iter())
            .find(|f| f.key == key)
            .map(|f| f.kind.clone())
    }

    /// The `settings.set` patch batch for the current staged edits.
    #[must_use]
    pub fn patches(&self) -> Vec<codeoid_protocol::SettingPatch> {
        self.dirty
            .iter()
            .map(|(key, value)| codeoid_protocol::SettingPatch {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeoid_protocol::{IdentityType, MessageIdentity};

    fn mk_session_info(id: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            name: "demo".into(),
            workdir: "/tmp".into(),
            status: codeoid_protocol::SessionStatus::Idle,
            created_by: "u".into(),
            created_at: "2026-06-23T00:00:00Z".into(),
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
            forked_from: None,
            worktree: None,
        }
    }

    #[test]
    fn prepend_growth_does_not_bump_the_bottom_anchored_offset() {
        let mut state = AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "u".into(),
                name: None,
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
            providers: None,
        });
        // Scrolled up, 100 rows rendered.
        state.note_total_rendered(100);
        state.scroll_up(10);
        assert_eq!(state.scroll_offset, 10);

        // Bottom growth (live streaming): anchored offset follows.
        state.note_total_rendered(120);
        assert_eq!(state.scroll_offset, 30);

        // Top growth (older-history prepend): offset must stay put — it's
        // measured from the BOTTOM, which didn't move.
        state.suppress_growth_anchor_once = true;
        state.note_total_rendered(200);
        assert_eq!(state.scroll_offset, 30, "prepend must not shift the view");
        assert!(!state.suppress_growth_anchor_once, "one-shot consumed");

        // Next bottom growth anchors again.
        state.note_total_rendered(210);
        assert_eq!(state.scroll_offset, 40);
    }

    #[test]
    fn mcp_tab_is_appended_after_manifest_tabs_when_servers_exist() {
        use codeoid_protocol::{McpServerStatus, SettingsManifest, SettingsSnapshot, SettingsTab};
        let mut m = SettingsModal::new();
        m.manifest = Some(SettingsManifest {
            version: 1,
            tabs: vec![SettingsTab {
                id: "general".into(),
                title: "General".into(),
                icon: None,
                description: None,
                groups: vec![],
            }],
        });
        // No snapshot → no synthetic MCP tab.
        assert!(!m.has_mcp_tab());
        assert_eq!(m.tab_count(), 1);

        // Snapshot with a registry server → the MCP tab is appended.
        m.snapshot = Some(SettingsSnapshot {
            values: HashMap::new(),
            secrets: HashMap::new(),
            config_path: "c".into(),
            env_path: "e".into(),
            mcp_servers: vec![McpServerStatus {
                name: "github".into(),
                transport: "stdio".into(),
                trust: "prompt".into(),
                scope: "workspace".into(),
                backends: None,
                enabled: true,
                builtin: false,
                health: "idle".into(),
                tool_count: 0,
                tools: vec![],
                error: None,
            }],
        });
        assert!(m.has_mcp_tab());
        assert_eq!(m.tab_count(), 2);

        // The last tab is the MCP tab; it's read-only, so it has no fields.
        m.tab = 1;
        assert!(m.on_mcp_tab());
        assert!(m.tab_fields().is_empty());
        assert!(m.selected_field().is_none());

        // Parked on the MCP tab when it vanishes: the index is stale until a
        // fresh snapshot triggers clamp_tab, which pulls it back into range.
        m.snapshot.as_mut().unwrap().mcp_servers.clear();
        assert!(!m.has_mcp_tab());
        assert_eq!(m.tab_count(), 1);
        assert!(!m.on_mcp_tab());
        assert_eq!(m.tab, 1); // stale
        m.clamp_tab();
        assert_eq!(m.tab, 0); // clamped to the last manifest tab
    }

    #[test]
    fn set_sessions_prunes_state_for_dead_sessions() {
        let mut state = AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "u".into(),
                name: None,
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
            providers: None,
        });
        // Two sessions with client-side state hanging off each.
        for sid in ["alive", "dead"] {
            state.sessions.upsert(mk_session_info(sid));
            state
                .messages
                .apply_message(tool_call_msg(sid, &format!("{sid}-m1")));
            state.note_attached(sid);
            state.mark_activity(sid);
        }

        // A history fetch was in flight for the doomed session.
        state.paging_in_flight = Some(("dead".into(), 0));

        // The daemon's next list only knows "alive" (dead was destroyed
        // in another client).
        state.set_sessions(vec![mk_session_info("alive")]);

        assert!(
            state.messages.messages("dead").is_empty(),
            "transcript freed"
        );
        assert!(!state.attached.contains("dead"));
        assert!(!state.activity_by_session.contains_key("dead"));
        // The global paging lock held by the destroyed session is released —
        // otherwise every other session is blocked from paging for 10s.
        assert!(
            state.paging_in_flight.is_none(),
            "dead session's paging lock must be released"
        );
        // Live session untouched.
        assert_eq!(state.messages.messages("alive").len(), 1);
        assert!(state.attached.contains("alive"));
    }

    #[test]
    fn prune_keeps_a_live_sessions_paging_lock() {
        let mut state = AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "u".into(),
                name: None,
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
            providers: None,
        });
        state.sessions.upsert(mk_session_info("alive"));
        state.paging_in_flight = Some(("alive".into(), 7));
        state.set_sessions(vec![mk_session_info("alive")]);
        assert_eq!(state.paging_in_flight, Some(("alive".into(), 7)));
    }

    fn tool_call_msg(sid: &str, mid: &str) -> codeoid_protocol::SessionMessage {
        codeoid_protocol::SessionMessage {
            session_id: sid.into(),
            message_id: mid.into(),
            role: codeoid_protocol::MessageRole::ToolCall,
            content: String::new(),
            parts: None,
            identity: MessageIdentity {
                sub: "spiffe://x/agent/t".into(),
                name: None,
                kind: IdentityType::Agent,
            },
            tool: Some(codeoid_protocol::ToolInfo {
                tool_id: "t1".into(),
                name: "Bash".into(),
                state: codeoid_protocol::ToolState::Completed {
                    success: true,
                    output: None,
                    elapsed_ms: Some(1),
                    confirmed_by: None,
                },
            }),
            metadata: None,
            timestamp: "2026-06-23T00:00:00Z".into(),
        }
    }

    fn mk_state() -> AppState {
        AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "spiffe://x".into(),
                name: Some("Test".into()),
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
            providers: None,
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
        assert!(
            !state.note_attached("s1"),
            "second call should return false"
        );
        assert!(state.note_attached("s2"));
    }

    #[test]
    fn tool_selection_evicts_only_the_focused_sessions_build() {
        let mut state = mk_state();
        state.sessions.upsert(mk_session_info("s1")); // auto-focuses s1
        state.sessions.upsert(mk_session_info("s2"));
        state.messages.apply_message(tool_call_msg("s1", "t-a"));
        // Seed cached builds for both sessions.
        let _ = state
            .scrollback_build
            .insert("s1".into(), scrollback_build::ScrollbackBuild::default());
        let _ = state
            .scrollback_build
            .insert("s2".into(), scrollback_build::ScrollbackBuild::default());

        state.cycle_tool_block_selection(true);

        assert!(
            state.scrollback_build.get("s1").is_none(),
            "focused session's build is stale after selection change"
        );
        assert!(
            state.scrollback_build.get("s2").is_some(),
            "selection is session-local — other sessions keep their builds"
        );
    }

    #[test]
    fn tool_expand_evicts_only_the_focused_sessions_build() {
        let mut state = mk_state();
        state.sessions.upsert(mk_session_info("s1"));
        state.sessions.upsert(mk_session_info("s2"));
        state.messages.apply_message(tool_call_msg("s1", "t-a"));
        let _ = state
            .scrollback_build
            .insert("s1".into(), scrollback_build::ScrollbackBuild::default());
        let _ = state
            .scrollback_build
            .insert("s2".into(), scrollback_build::ScrollbackBuild::default());

        state.toggle_expand_selected_tool_block();

        assert!(state.expanded_tool_message_ids.contains("t-a"));
        assert!(state.scrollback_build.get("s1").is_none());
        assert!(
            state.scrollback_build.get("s2").is_some(),
            "expand is session-local — other sessions keep their builds"
        );
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
        // scroll_to_top sets offset to usize::MAX; the renderer's
        // saturating math turns that into row 0. Here we just verify
        // the state-side contract.
        let mut state = mk_state();
        state.scroll_to_top();
        assert_eq!(state.scroll_offset, usize::MAX);
    }

    #[test]
    fn scroll_offset_exceeds_former_u16_ceiling() {
        // Regression: scroll_offset was u16, capping scrollback at
        // 65 535 wrapped rows — the top of big sessions was unreachable.
        let mut state = mk_state();
        state.note_total_rendered(200_000);
        state.scroll_up(70_000);
        assert_eq!(state.scroll_offset, 70_000);

        // Anchored maintenance across the old ceiling: new rows at the
        // bottom keep bumping the offset with no saturation at 65 535.
        state.note_total_rendered(300_000);
        assert_eq!(state.scroll_offset, 170_000);
        assert_eq!(state.unseen_below_rows, 100_000);

        // The renderer's `y = max_y - offset` math: with viewport 50,
        // max_y = 299_950 and every row above the old ceiling is
        // reachable.
        let max_y = 300_000usize.saturating_sub(50);
        assert_eq!(max_y.saturating_sub(state.scroll_offset), 129_950);

        // And scroll_to_top from here pins to row 0.
        state.scroll_to_top();
        assert_eq!(max_y.saturating_sub(state.scroll_offset), 0);
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

    // ── Provider dialogs + commands ───────────────────────────────────────

    fn mk_ui_request(sid: &str, rid: &str) -> SessionUiRequestMsg {
        SessionUiRequestMsg {
            session_id: sid.into(),
            request_id: rid.into(),
            method: UiRequestMethod::Select,
            title: "Pick".into(),
            message: None,
            options: Some(vec!["a".into(), "b".into(), "c".into()]),
            placeholder: None,
            prefill: None,
            timeout_ms: None,
            timestamp: "t".into(),
        }
    }

    #[test]
    fn ui_requests_dedupe_and_keep_arrival_order() {
        let mut state = mk_state();
        state.add_ui_request(mk_ui_request("s1", "a"));
        state.add_ui_request(mk_ui_request("s1", "b"));
        // Attach re-delivery — must not duplicate.
        state.add_ui_request(mk_ui_request("s1", "a"));
        assert_eq!(state.pending_ui_requests["s1"].len(), 2);
        assert_eq!(state.pending_ui_requests["s1"][0].request_id, "a");
    }

    #[test]
    fn maybe_open_ui_dialog_opens_head_for_focused_session_only() {
        let mut state = mk_state();
        state.sessions.replace(vec![mk_session_info("s1")]);
        state.sessions.focus_id("s1");
        state.add_ui_request(mk_ui_request("other", "x"));
        state.maybe_open_ui_dialog();
        assert!(
            state.modal.is_none(),
            "other session's dialog must not open"
        );

        state.add_ui_request(mk_ui_request("s1", "a"));
        state.maybe_open_ui_dialog();
        match state.modal.as_ref() {
            Some(Modal::UiDialog(m)) => assert_eq!(m.request.request_id, "a"),
            other => panic!("expected UiDialog, got {other:?}"),
        }
    }

    #[test]
    fn maybe_open_never_steals_an_existing_modal() {
        let mut state = mk_state();
        state.sessions.replace(vec![mk_session_info("s1")]);
        state.sessions.focus_id("s1");
        state.modal = Some(Modal::Help);
        state.add_ui_request(mk_ui_request("s1", "a"));
        state.maybe_open_ui_dialog();
        assert!(matches!(state.modal, Some(Modal::Help)));
    }

    #[test]
    fn remove_ui_request_closes_matching_modal_and_reveals_next() {
        let mut state = mk_state();
        state.sessions.replace(vec![mk_session_info("s1")]);
        state.sessions.focus_id("s1");
        state.add_ui_request(mk_ui_request("s1", "a"));
        state.add_ui_request(mk_ui_request("s1", "b"));
        state.maybe_open_ui_dialog();

        // Resolved elsewhere (another client answered / timeout).
        state.remove_ui_request("s1", "a");
        assert!(state.modal.is_none(), "resolved dialog must close");
        state.maybe_open_ui_dialog();
        match state.modal.as_ref() {
            Some(Modal::UiDialog(m)) => assert_eq!(m.request.request_id, "b"),
            other => panic!("expected next dialog, got {other:?}"),
        }
    }

    #[test]
    fn ui_dialog_modal_navigation_and_text_entry() {
        let mut m = UiDialogModal::new(mk_ui_request("s1", "a"));
        assert!(!m.is_text_entry());
        assert_eq!(m.selected_option(), Some("a"));
        m.next_option();
        assert_eq!(m.selected_option(), Some("b"));
        m.prev_option();
        m.prev_option();
        assert_eq!(m.selected_option(), Some("c"), "prev wraps");

        let mut req = mk_ui_request("s1", "b");
        req.method = UiRequestMethod::Editor;
        req.prefill = Some("draft".into());
        req.options = None;
        let text = UiDialogModal::new(req);
        assert!(text.is_text_entry());
        assert_eq!(text.buffer, "draft");
        assert_eq!(text.selected_option(), None);
    }

    #[test]
    fn provider_command_lookup_is_focused_session_scoped_and_case_insensitive() {
        let mut state = mk_state();
        state.sessions.replace(vec![mk_session_info("s1")]);
        state.sessions.focus_id("s1");
        state.provider_commands.insert(
            "s1".into(),
            vec![ProviderCommand {
                name: "Fix-Tests".into(),
                description: None,
                source: Some("extension".into()),
                argument_hint: None,
            }],
        );
        state.provider_commands.insert(
            "other".into(),
            vec![ProviderCommand {
                name: "deploy".into(),
                description: None,
                source: None,
                argument_hint: None,
            }],
        );
        assert!(state.is_provider_command("fix-tests"));
        assert!(state.is_provider_command("FIX-TESTS"));
        assert!(
            !state.is_provider_command("deploy"),
            "other session's catalog"
        );
        assert_eq!(state.focused_provider_commands().len(), 1);
    }
}
