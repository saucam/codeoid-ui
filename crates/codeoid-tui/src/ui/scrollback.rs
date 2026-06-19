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

    // Animated content (running tool spinners, elapsed-time counters)
    // must repaint every tick. If any focused-session message is
    // animating, force a rebuild so the spinner actually advances.
    let any_animating = state
        .messages
        .messages(&session_id)
        .iter()
        .any(is_animating);

    let cache_hit = !any_animating
        && state
            .scrollback_build
            .matches(&session_id, inner_width, epoch);

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
        for m in msgs {
            let skip_cache = is_animating(m);
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

        // Use Paragraph's own line_count so we never disagree with the
        // widget that actually lays out the content — a hand-rolled row
        // counter that under-reports by even one row clips the bottom
        // of the transcript at scroll_offset = 0.
        let total = Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .line_count(inner_width);
        scrollback_build.session_id = Some(session_id.clone());
        scrollback_build.width = inner_width;
        scrollback_build.epoch = epoch;
        scrollback_build.lines = lines;
        scrollback_build.total_rendered_rows = total;
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
    let y = max_y
        .saturating_sub(state.scroll_offset as usize)
        .min(u16::MAX as usize) as u16;

    // Paragraph::new requires `Vec<Line>` by value, so we clone the
    // outer Vec from the cache. The Tier 2 / custom-widget path
    // eliminates this clone entirely, but it's already an order of
    // magnitude cheaper than re-running the per-message renderer.
    let lines = state.scrollback_build.lines.clone();
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((y, 0))
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
