//! Transcript viewport. Renders every message for the focused session
//! with role-aware styling, right-aligned timestamps, and a live
//! "Thinking…" placeholder for the in-flight assistant message.

use codeoid_protocol::{MessageRole, SessionInfo, SessionMessage, ToolState};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::render::{parse_ansi, render_markdown_block, render_tool_block, sanitize_for_display};
use crate::state::{AppState, Focus};

const BODY_INDENT: &str = "  ";

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &mut AppState) {
    let focused_pane = state.focus == Focus::Scrollback;
    let border_style = if focused_pane {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Snapshot the focused session's id + title so we can release the
    // immutable borrow on `state.sessions` before reborrowing other
    // fields mutably below.
    let session_snapshot = state
        .sessions
        .focused()
        .map(|s| (s.id.clone(), session_title(s)));

    let Some((session_id, title)) = session_snapshot else {
        let placeholder = Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "no session",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(" Transcript "),
        );
        frame.render_widget(placeholder, area);
        return;
    };

    let anim_tick = state.anim_tick;
    let verbose_tools = state.verbose_tool_output;
    let expanded_ids = state.expanded_tool_message_ids.clone();
    let selected_id = state.selected_tool_message_id.clone();
    let inner_width = area.width.saturating_sub(2).max(1); // minus L+R border
    let viewport_rows_u16 = area.height.saturating_sub(2);
    let viewport_rows: usize = viewport_rows_u16.into(); // minus T+B border
    state.last_viewport_rows = viewport_rows_u16;

    // Decide whether the frame-to-frame assembled-lines cache is usable.
    // Hits eliminate the per-message walk + per-message render-cache
    // lookup + total-rendered-rows recomputation entirely. Tier 1 of the
    // perf plan — cache miss falls back to the same rebuild path the
    // renderer used unconditionally before.
    let epoch = state.messages.epoch_of_session(&session_id);

    // Structural check is O(1). The O(N) animation scan is only needed
    // when the structural key already matches — there's no value in
    // scanning all messages when we already know the session or width
    // changed. And when `has_animating` is false (idle session), we skip
    // the scan entirely even on structural hits.
    let structural_hit = state
        .scrollback_build
        .matches(&session_id, inner_width, epoch);

    let cache_hit = structural_hit && {
        if state.scrollback_build.has_animating {
            // Spinners were active last build — scan to see if any still run.
            !state
                .messages
                .messages(&session_id)
                .iter()
                .any(is_animating)
        } else {
            // No animations last build + same epoch → definite cache hit.
            true
        }
    };

    if !cache_hit {
        // Rebuild: split-borrow `messages` + `render_cache` +
        // `scrollback_build` (disjoint fields, so Rust accepts this
        // simultaneously via the destructuring pattern).
        let AppState {
            ref messages,
            ref mut render_cache,
            ref mut scrollback_build,
            ..
        } = *state;

        let msgs = messages.messages(&session_id);

        // Bound the per-message render cache to the focused session's
        // live ids. Without this it grows monotonically across
        // rotations / session switches and `apply_delta` lookups
        // gradually slow down — visible as prompt lag after many turns.
        let live_ids: std::collections::HashSet<String> =
            msgs.iter().map(|m| m.message_id.clone()).collect();
        render_cache.retain_only(&live_ids);

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(msgs.len() * 4);
        let mut any_animating = false;
        for m in msgs {
            let skip_cache = is_animating(m);
            if skip_cache {
                any_animating = true;
            }
            let version = messages.version_of(&m.message_id);
            let per_block_expanded = expanded_ids.contains(&m.message_id);
            let is_selected = selected_id.as_deref() == Some(m.message_id.as_str());
            let rendered =
                render_cache.get_or_render(&m.message_id, version, inner_width, skip_cache, || {
                    render_message(
                        m,
                        anim_tick,
                        verbose_tools || per_block_expanded,
                        is_selected,
                    )
                });
            if rendered.is_empty() {
                // Placeholder messages (empty assistant/thinking
                // mid-stream) don't render in the transcript — the
                // worker row above the prompt is the single source of
                // "something is happening".
                continue;
            }
            lines.extend(rendered);
            lines.push(Line::raw(""));
        }

        if msgs.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::raw(BODY_INDENT.to_string()),
                Span::styled(
                    "No messages yet. Type below and press Enter.",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        }

        // Per-logical-line wrapped-row prefix sum. `count_wrapped_rows`
        // replaces the old `Paragraph::new(vec![line.clone()]).line_count()`
        // call — same O(chars) Unicode-width walk but without allocating a
        // Paragraph widget + running ratatui's layout pass per line. Built
        // once per cache miss; the render path below never walks the whole
        // buffer again.
        let mut row_offsets: Vec<usize> = Vec::with_capacity(lines.len() + 1);
        let mut acc = 0usize;
        row_offsets.push(0);
        for line in &lines {
            acc += count_wrapped_rows(line, inner_width);
            row_offsets.push(acc);
        }
        scrollback_build.session_id = Some(session_id.clone());
        scrollback_build.width = inner_width;
        scrollback_build.epoch = epoch;
        scrollback_build.has_animating = any_animating;
        scrollback_build.lines = lines;
        scrollback_build.total_rendered_rows = acc;
        scrollback_build.row_offsets = row_offsets;
    }

    // Scroll math reuses the precomputed total. While the user is
    // scrolled up (Anchored mode), `note_total_rendered` bumps
    // `scroll_offset` by however many rows arrived at the bottom since
    // the previous frame, so the visible window stays pinned to the
    // content the user was reading. Bottom mode (offset = 0) just
    // follows the latest row.
    let total_rendered = state.scrollback_build.total_rendered_rows;
    state.note_total_rendered(total_rendered);

    let max_y = total_rendered.saturating_sub(viewport_rows);
    let y = max_y.saturating_sub(state.scroll_offset as usize);

    // Hand ratatui only the logical lines that intersect the viewport,
    // not the whole transcript. This is what keeps scrolling O(viewport):
    // a 10k-line session re-wraps ~viewport rows per frame instead of all
    // 10k. `y` is the top visible wrapped row; the slice + intra-line
    // scroll offset reproduce exactly that window.
    let (first, intra, last) = crate::state::scrollback_build::visible_window(
        &state.scrollback_build.row_offsets,
        y,
        viewport_rows,
    );
    let window: Vec<Line<'static>> = state.scrollback_build.lines[first..last].to_vec();
    let paragraph = Paragraph::new(window)
        .wrap(Wrap { trim: false })
        .scroll((intra.min(u16::MAX as usize) as u16, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        );

    frame.render_widget(paragraph, area);
}

/// True when this message's rendered appearance changes per anim_tick.
/// Today: running-tool spinners + elapsed-time counters. Cache must be
/// bypassed for these so the spinner actually moves.
fn is_animating(m: &SessionMessage) -> bool {
    m.tool.as_ref().is_some_and(|t| {
        matches!(
            &t.state,
            ToolState::Streaming { .. } | ToolState::Executing { .. }
        )
    })
}

fn session_title(session: &SessionInfo) -> Line<'static> {
    let mode = session.mode.map_or("interactive", |m| match m {
        codeoid_protocol::SessionMode::Interactive => "interactive",
        codeoid_protocol::SessionMode::Guarded => "guarded",
        codeoid_protocol::SessionMode::Autonomous => "autonomous",
    });
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            session.name.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", session.workdir),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  · "),
        Span::styled(
            mode.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
        Span::raw(" "),
    ])
}

/// Render a session message as styled lines. Returns empty when there's
/// nothing worth drawing yet (empty assistant/thinking placeholders
/// between deltas) — the worker row above the prompt carries the
/// session-level "thinking" signal so the transcript doesn't need a
/// duplicate spinner. The tool card's per-tool phase indicator IS kept,
/// since it conveys per-tool lifecycle (which tool is still running),
/// not session-level busyness.
fn render_message(
    m: &SessionMessage,
    anim_tick: u64,
    verbose_tools: bool,
    is_selected: bool,
) -> Vec<Line<'static>> {
    // Skip placeholder messages that will be filled in by streaming
    // deltas. The header reappears once content or parts arrive.
    let has_payload = !m.content.is_empty()
        || m.parts.as_ref().is_some_and(|p| !p.is_empty())
        || m.tool.is_some();
    if !has_payload {
        return Vec::new();
    }

    let mut out = Vec::new();
    out.push(role_header(m));

    match m.role {
        MessageRole::ToolCall => {
            if let Some(tool) = &m.tool {
                out.extend(render_tool_block(
                    tool,
                    anim_tick,
                    BODY_INDENT,
                    verbose_tools,
                    is_selected,
                ));
            }
            if !m.content.is_empty() {
                out.extend(render_markdown_block(&m.content, BODY_INDENT));
            }
        }
        MessageRole::ToolResult => {
            // Preserve ANSI styling from the tool's raw output so git /
            // cargo / npm colors survive. A magenta left-rail marks each
            // row as tool output without masking the command's own colors.
            // In collapsed (default) mode we cap at 8 lines + a hint row;
            // press `v` to expand. Mirrors the limit used in
            // `render_tool_block` for the inline tool-call output body.
            let rail_style = Style::default().fg(Color::Magenta);
            let lines = parse_ansi(&m.content);
            let cap = if verbose_tools { 40 } else { 8 };
            let total = lines.len();
            for line in lines.into_iter().take(cap) {
                let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
                spans.push(Span::raw(BODY_INDENT.to_string()));
                spans.push(Span::styled("▌ ", rail_style));
                spans.extend(line.spans);
                out.push(Line::from(spans));
            }
            if total > cap {
                let hint = if verbose_tools {
                    format!("… {} more line(s) truncated", total - cap)
                } else {
                    format!(
                        "… {} more line(s) hidden — press v to expand tool output",
                        total - cap,
                    )
                };
                out.push(Line::from(vec![
                    Span::raw(BODY_INDENT.to_string()),
                    Span::styled("▌ ", rail_style),
                    Span::styled(
                        hint,
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        }
        MessageRole::Thinking => {
            let clean = sanitize_for_display(&m.content);
            for raw in clean.lines() {
                out.push(Line::from(vec![
                    Span::raw(BODY_INDENT.to_string()),
                    Span::styled("◇ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        raw.to_string(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        }
        MessageRole::System | MessageRole::Info => {
            let clean = sanitize_for_display(&m.content);
            for raw in clean.lines() {
                let style = if matches!(m.role, MessageRole::System) {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                out.push(Line::from(vec![
                    Span::raw(BODY_INDENT.to_string()),
                    Span::styled(raw.to_string(), style),
                ]));
            }
        }
        MessageRole::User => {
            let clean = sanitize_for_display(&m.content);
            for raw in clean.lines() {
                out.push(Line::from(vec![
                    Span::raw(BODY_INDENT.to_string()),
                    Span::styled(raw.to_string(), Style::default().fg(Color::White)),
                ]));
            }
        }
        MessageRole::Assistant => {
            // Markdown renderer walks characters, so raw ANSI would
            // corrupt parsing just as badly. Strip before parse.
            let clean = sanitize_for_display(&m.content);
            out.extend(render_markdown_block(&clean, BODY_INDENT));
        }
    }

    out
}

fn role_header(m: &SessionMessage) -> Line<'static> {
    let (label, style) = match m.role {
        MessageRole::User => (
            "you",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        MessageRole::Assistant => (
            "assistant",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        MessageRole::Thinking => (
            "reasoning",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
        MessageRole::ToolCall => (
            "tool",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        MessageRole::ToolResult => ("tool output", Style::default().fg(Color::Magenta)),
        MessageRole::System => (
            "system",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        MessageRole::Info => ("info", Style::default().fg(Color::DarkGray)),
    };

    let identity = m
        .identity
        .name
        .clone()
        .unwrap_or_else(|| short_sub(&m.identity.sub));

    let time = fmt_timestamp(&m.timestamp);

    // Left side: ▎ role  identity    Right: time (padded with spaces so
    // Wrap doesn't clip the timestamp).
    Line::from(vec![
        Span::styled("▎ ", style),
        Span::styled(label.to_string(), style),
        Span::raw("  "),
        Span::styled(
            identity,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(format!("   {time}"), Style::default().fg(Color::DarkGray)),
    ])
}

/// Render an ISO-8601 / RFC-3339 timestamp as HH:MM:SS. Best-effort; falls
/// back to the trailing 8 chars if parsing fails.
fn fmt_timestamp(raw: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt.format("%H:%M:%S").to_string(),
        Err(_) => raw
            .chars()
            .rev()
            .take(8)
            .collect::<String>()
            .chars()
            .rev()
            .collect(),
    }
}

fn short_sub(sub: &str) -> String {
    // spiffe://…/agent/<name> → <name>, else tail path segment.
    sub.rsplit('/').next().unwrap_or(sub).to_string()
}

/// Compute the number of terminal rows a single logical line occupies at
/// `width` columns. Uses ratatui's `Line::width()` (sum of Unicode display
/// widths across all spans) divided by the viewport width — no Paragraph
/// allocation or layout pass, so this is cheap to call for every line in
/// a long transcript.
///
/// The result is a slight over-estimate for lines whose longest word
/// exceeds `width` (word-wrap pushes the word to the next row, leaving
/// the previous row shorter than `width`). For practical transcript
/// content this is rare and the error is at most one row per message,
/// which is acceptable for scroll math.
fn count_wrapped_rows(line: &Line<'_>, width: u16) -> usize {
    let w = width as usize;
    if w == 0 {
        return 1;
    }
    let cell_width = line.width();
    if cell_width == 0 { 1 } else { cell_width.div_ceil(w) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeoid_protocol::{AuthOkMsg, IdentityType, MessageIdentity, SessionStatus};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;
    use ratatui::Terminal;

    fn mk_state() -> AppState {
        AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "spiffe://x".into(),
                name: Some("Me".into()),
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
        })
    }

    fn mk_session(id: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            name: "demo".into(),
            workdir: "/tmp".into(),
            status: SessionStatus::Idle,
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
        }
    }

    fn user_msg(sid: &str, id: &str, content: &str) -> SessionMessage {
        SessionMessage {
            session_id: sid.into(),
            message_id: id.into(),
            role: MessageRole::User,
            content: content.into(),
            parts: None,
            identity: MessageIdentity {
                sub: "spiffe://x/agent/t".into(),
                name: None,
                kind: IdentityType::Agent,
            },
            tool: None,
            metadata: None,
            timestamp: "2026-06-23T00:00:00Z".into(),
        }
    }

    fn buf_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(Cell::symbol)
            .collect()
    }

    #[test]
    fn builds_prefix_sum_and_shows_latest_at_bottom() {
        let mut state = mk_state();
        state.sessions.upsert(mk_session("s1")); // auto-focuses
        for i in 0..5 {
            state.messages.apply_message(user_msg(
                "s1",
                &format!("m{i}"),
                &format!("hello message {i}"),
            ));
        }
        let mut terminal = Terminal::new(TestBackend::new(40, 14)).unwrap();
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();

        // The cache miss built the prefix sum; its last entry is the total.
        let off = &state.scrollback_build.row_offsets;
        assert!(off.len() >= 2, "prefix sum not built");
        assert_eq!(
            *off.last().unwrap(),
            state.scrollback_build.total_rendered_rows
        );

        // Following the bottom → the most recent message is on screen.
        assert!(
            buf_text(&terminal).contains("hello message 4"),
            "latest message should be visible"
        );
    }

    #[test]
    fn windowed_render_follows_scroll_offset() {
        let mut state = mk_state();
        state.sessions.upsert(mk_session("s1"));
        for i in 0..40 {
            state
                .messages
                .apply_message(user_msg("s1", &format!("m{i}"), &format!("LINE{i:02}")));
        }
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        // First render populates total + row_offsets at the bottom.
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();
        // Scroll to the very top; the window must now show the earliest lines
        // and not the latest — i.e. the slice tracked the offset.
        state.scroll_offset = u16::MAX;
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();

        let text = buf_text(&terminal);
        assert!(
            text.contains("LINE00"),
            "top of transcript should be visible when scrolled up: {text}"
        );
        assert!(
            !text.contains("LINE39"),
            "the latest line must be off-screen when scrolled to the top"
        );
    }
}
