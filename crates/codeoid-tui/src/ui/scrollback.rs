//! Transcript viewport. Renders every message for the focused session
//! with role-aware styling, right-aligned timestamps, and a live
//! "Thinking…" placeholder for the in-flight assistant message.

use codeoid_protocol::{MessageRole, SessionInfo, SessionMessage, ToolState};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::render::{
    has_rich_parts, parse_ansi, render_markdown_block, render_parts, render_tool_block,
    sanitize_for_display,
};
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
    // must repaint every tick even though the message content (and thus
    // the session epoch) is unchanged.
    let any_animating = state
        .messages
        .messages(&session_id)
        .iter()
        .any(is_animating);

    let build_matches = state
        .scrollback_build
        .matches(&session_id, inner_width, epoch);

    if build_matches {
        // Mark the session most-recently-focused so LRU eviction
        // tracks focus recency, not just rebuild recency.
        state.scrollback_build.touch(&session_id);
    }

    if build_matches && any_animating {
        // Animation-only frame: same content (epoch unchanged), same
        // width — only the animating messages' spinner/elapsed lines
        // look different. Re-render JUST those messages and splice them
        // into the cached build. This is what keeps a running tool at
        // 10 Hz from re-cloning and re-measuring the whole transcript
        // every tick: cost is O(animating lines), not O(total lines).
        let AppState {
            ref messages,
            ref mut scrollback_build,
            ..
        } = *state;
        if let Some(build) = scrollback_build.get_mut(&session_id) {
            for m in messages
                .messages(&session_id)
                .iter()
                .filter(|m| is_animating(m))
            {
                let per_block_expanded = expanded_ids.contains(&m.message_id);
                let is_selected = selected_id.as_deref() == Some(m.message_id.as_str());
                let mut new_lines = render_message(
                    m,
                    anim_tick,
                    verbose_tools || per_block_expanded,
                    is_selected,
                );
                if !new_lines.is_empty() {
                    // Trailing separator, exactly as the full rebuild
                    // appends per rendered message.
                    new_lines.push(Line::raw(""));
                }
                build.splice_message(&m.message_id, new_lines);
            }
        }
    } else if !build_matches {
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

        // Bound THIS session's per-message render cache to its live
        // ids. Without this it grows monotonically across rotations and
        // scrollback replaces. Scoped to the session on purpose: the
        // old retain-across-everything variant evicted every OTHER
        // session's cached renders on focus switch, making each Tab an
        // O(N) re-parse of the target session.
        let live_ids: std::collections::HashSet<String> =
            msgs.iter().map(|m| m.message_id.clone()).collect();
        render_cache.retain_session(&session_id, &live_ids);

        // INVARIANT: message ids are unique within a session, so keying
        // the render cache and BuildSegments by `message_id` alone is
        // sound. The daemon mints every messageId with randomUUID() and
        // its ScrollbackBuffer.push upserts by messageId precisely so
        // scrollback.replay can never carry two entries with the same
        // id ("the #50 bug class" in codeoid's session.ts). On this
        // side, MessageStore::apply_message upserts by id and
        // apply_delta patches in place — neither can introduce a
        // duplicate either. Newest-wins resolution in both stores keeps
        // even a hypothetical protocol violation consistent rather than
        // corrupt.
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(msgs.len() * 4);
        let mut row_counts: Vec<usize> = Vec::with_capacity(msgs.len() * 4);
        let mut segments: Vec<crate::state::scrollback_build::BuildSegment> =
            Vec::with_capacity(msgs.len());
        for m in msgs {
            let skip_cache = is_animating(m);
            let version = messages.version_of(&m.message_id);
            let per_block_expanded = expanded_ids.contains(&m.message_id);
            let is_selected = selected_id.as_deref() == Some(m.message_id.as_str());
            // Cached messages come back with their wrapped-row counts
            // measured once at store time — the rebuild never re-wraps
            // unchanged content, so a streaming session rebuilds in
            // O(changed lines) measurement, not O(transcript).
            let (rendered, counts) = render_cache.get_or_render(
                &session_id,
                &m.message_id,
                version,
                inner_width,
                skip_cache,
                || {
                    render_message(
                        m,
                        anim_tick,
                        verbose_tools || per_block_expanded,
                        is_selected,
                    )
                },
            );
            if rendered.is_empty() {
                // Placeholder messages (empty assistant/thinking
                // mid-stream) don't render in the transcript — the
                // worker row above the prompt is the single source of
                // "something is happening".
                continue;
            }
            let first_line = lines.len();
            lines.extend(rendered);
            row_counts.extend(counts);
            lines.push(Line::raw(""));
            row_counts.push(1); // separator: empty line = 1 row at any width
            segments.push(crate::state::scrollback_build::BuildSegment {
                message_id: m.message_id.clone(),
                first_line,
                line_count: lines.len() - first_line,
            });
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
            for line in &lines {
                row_counts.push(crate::state::scrollback_build::wrapped_row_count(
                    line,
                    inner_width,
                ));
            }
        }

        // Per-logical-line wrapped-row prefix sum, assembled from the
        // cached per-message counts (ratatui wraps each Line
        // independently, so per-line counts sum to the whole-buffer
        // count — scroll math stays byte-for-byte consistent with how
        // the windowed slice is laid out on render, and never
        // under-reports the bottom row).
        let mut build = crate::state::scrollback_build::ScrollbackBuild {
            width: inner_width,
            epoch,
            lines,
            total_rendered_rows: 0,
            row_counts,
            row_offsets: Vec::new(),
            segments,
        };
        build.rebuild_offsets();
        // A session pushed off the LRU takes its per-message render
        // cache with it — that's what bounds total memory across many
        // sessions.
        if let Some(evicted) = scrollback_build.insert(session_id.clone(), build) {
            render_cache.evict_session(&evicted);
        }
    }

    // Scroll math reuses the precomputed total. While the user is
    // scrolled up (Anchored mode), `note_total_rendered` bumps
    // `scroll_offset` by however many rows arrived at the bottom since
    // the previous frame, so the visible window stays pinned to the
    // content the user was reading. Bottom mode (offset = 0) just
    // follows the latest row.
    let total_rendered = state
        .scrollback_build
        .get(&session_id)
        .map_or(0, |b| b.total_rendered_rows);
    state.note_total_rendered(total_rendered);

    let max_y = total_rendered.saturating_sub(viewport_rows);
    // Write the clamp back: `scroll_to_top` parks the offset at usize::MAX
    // and holding ↑ at the top keeps saturating upward — without this,
    // every subsequent ScrollDown is a dead decrement somewhere in the
    // ~10^19 range (scroll-down "stops working" and the ↑N hint shows a
    // garbage number).
    state.scroll_offset = state.scroll_offset.min(max_y);
    let y = max_y.saturating_sub(state.scroll_offset);

    // Hand ratatui only the logical lines that intersect the viewport,
    // not the whole transcript. This is what keeps scrolling O(viewport):
    // a 10k-line session re-wraps ~viewport rows per frame instead of all
    // 10k. `y` is the top visible wrapped row; the slice + intra-line
    // scroll offset reproduce exactly that window.
    let Some(build) = state.scrollback_build.get(&session_id) else {
        // Unreachable: the branch above always inserts a build for the
        // focused session. Render nothing rather than panic.
        return;
    };
    let (first, intra, last) =
        crate::state::scrollback_build::visible_window(&build.row_offsets, y, viewport_rows);
    let window: Vec<Line<'static>> = build.lines[first..last].to_vec();
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
        // Non-default backend gets a visible tag — a mixed claude/pi fleet
        // must be tellable apart from the transcript header alone.
        Span::styled(
            match session.provider_id.as_deref() {
                Some(provider) if provider != "claude" => format!("  · {provider}"),
                _ => String::new(),
            },
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
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

    // Rich provider content (custom_message parts) wins over the plain
    // `content` fallback — but never over tool chrome, which owns its own
    // rendering. A single mirrored text part keeps the legacy paths (their
    // markdown/ANSI handling is tuned per role).
    if !matches!(m.role, MessageRole::ToolCall | MessageRole::ToolResult)
        && has_rich_parts(m.parts.as_ref())
    {
        if let Some(parts) = &m.parts {
            out.extend(render_parts(parts, BODY_INDENT));
            return out;
        }
    }

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
            capabilities: None,
            providers: None,
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
            provider_id: None,
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
        let build = state.scrollback_build.get("s1").expect("build cached");
        let off = &build.row_offsets;
        assert!(off.len() >= 2, "prefix sum not built");
        assert_eq!(*off.last().unwrap(), build.total_rendered_rows);

        // Following the bottom → the most recent message is on screen.
        assert!(
            buf_text(&terminal).contains("hello message 4"),
            "latest message should be visible"
        );
    }

    fn tool_msg(sid: &str, id: &str, state: ToolState) -> SessionMessage {
        SessionMessage {
            session_id: sid.into(),
            message_id: id.into(),
            role: MessageRole::ToolCall,
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
                state,
            }),
            metadata: None,
            timestamp: "2026-06-23T00:00:00Z".into(),
        }
    }

    #[test]
    fn animation_frame_splices_instead_of_rebuilding() {
        let mut state = mk_state();
        state.sessions.upsert(mk_session("s1"));
        state
            .messages
            .apply_message(user_msg("s1", "m0", "static message"));
        state.messages.apply_message(tool_msg(
            "s1",
            "m1",
            ToolState::Executing {
                progress: None,
                elapsed_ms: Some(100),
            },
        ));

        let mut terminal = Terminal::new(TestBackend::new(50, 14)).unwrap();
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();
        let spinner_t0 = crate::render::spinner::SpinnerFrame::for_tick(state.anim_tick).glyph();
        assert!(buf_text(&terminal).contains(spinner_t0));

        // Plant a sentinel into the CACHED build for the static
        // message's line. A full rebuild would overwrite it from the
        // render cache; an animation-only splice must leave it alone.
        {
            let build = state.scrollback_build.get_mut("s1").expect("build cached");
            let idx = build
                .lines
                .iter()
                .position(|l| l.spans.iter().any(|s| s.content.contains("static message")))
                .expect("static message line in build");
            build.lines[idx] = Line::raw("SENTINEL-NOT-REBUILT");
        }

        // Advance the animation and render again: the spinner must
        // move (animating message re-rendered + spliced) while the
        // sentinel survives (everything else untouched).
        state.tick();
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();
        let text = buf_text(&terminal);
        let spinner_t1 = crate::render::spinner::SpinnerFrame::for_tick(state.anim_tick).glyph();
        assert_ne!(spinner_t0, spinner_t1, "test needs distinct glyphs");
        assert!(
            text.contains(spinner_t1),
            "spinner must advance on an animation frame"
        );
        assert!(
            text.contains("SENTINEL-NOT-REBUILT"),
            "animation frame must splice, not rebuild the whole transcript"
        );

        // Tool completes (delta bumps the session epoch) → full
        // rebuild: sentinel replaced by the real cached render.
        let mut d = codeoid_protocol::SessionMessageDelta {
            session_id: "s1".into(),
            message_id: "m1".into(),
            content_append: None,
            parts_append: None,
            parts_update: None,
            tool_state_update: None,
            timestamp: "2026-06-23T00:00:02Z".into(),
        };
        d.tool_state_update = Some(ToolState::Completed {
            success: true,
            output: Some("done".into()),
            elapsed_ms: Some(250),
            confirmed_by: None,
        });
        state.messages.apply_delta(d);
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();
        let text = buf_text(&terminal);
        assert!(
            !text.contains("SENTINEL-NOT-REBUILT"),
            "tool-state transition must trigger a full rebuild"
        );
        assert!(text.contains("static message"));
    }

    #[test]
    fn focus_switch_keeps_other_sessions_render_cache() {
        // Regression: retention used to keep only the FOCUSED session's
        // ids, so every Tab evicted every other session's cached
        // renders and switching back was a full O(N) re-parse.
        let mut state = mk_state();
        state.sessions.upsert(mk_session("s1")); // auto-focuses s1
        state.sessions.upsert(mk_session("s2"));
        for i in 0..3 {
            state
                .messages
                .apply_message(user_msg("s1", &format!("a{i}"), "from s1"));
            state
                .messages
                .apply_message(user_msg("s2", &format!("b{i}"), "from s2"));
        }

        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();
        assert!(state.render_cache.contains("s1", "a0"));

        // Focus s2 and render — s1's cached renders must survive, and
        // s1's assembled build must still be a hit for a straight
        // A→B→A tab flip.
        state.sessions.focus_id("s2");
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();
        assert!(state.render_cache.contains("s2", "b0"));
        assert!(
            state.render_cache.contains("s1", "a0"),
            "focus switch must not evict other sessions' cached renders"
        );
        let width = 40 - 2; // minus L+R border
        assert!(
            state
                .scrollback_build
                .matches("s1", width, state.messages.epoch_of_session("s1")),
            "s1's assembled build should still be cached"
        );
    }

    #[test]
    fn render_cache_evicted_beyond_lru_window() {
        // Rotate focus through LRU_SESSIONS + 1 sessions; the least-
        // recently-focused one loses both its build and its per-message
        // render cache entries (memory bound).
        let n = crate::state::scrollback_build::LRU_SESSIONS + 1;
        let mut state = mk_state();
        for i in 0..n {
            let sid = format!("s{i}");
            state.sessions.upsert(mk_session(&sid));
            state
                .messages
                .apply_message(user_msg(&sid, &format!("m{i}"), "hello"));
        }

        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        for i in 0..n {
            state.sessions.focus_id(&format!("s{i}"));
            terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();
        }

        assert!(
            !state.render_cache.contains("s0", "m0"),
            "least-recently-focused session must be evicted with its build"
        );
        for i in 1..n {
            assert!(
                state
                    .render_cache
                    .contains(&format!("s{i}"), &format!("m{i}")),
                "s{i} is inside the LRU window and must be retained"
            );
        }
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
        state.scroll_offset = usize::MAX;
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

    #[test]
    fn scroll_offset_clamps_to_content_so_scroll_down_recovers() {
        let mut state = mk_state();
        state.sessions.upsert(mk_session("s1"));
        for i in 0..40 {
            state
                .messages
                .apply_message(user_msg("s1", &format!("m{i}"), &format!("LINE{i:02}")));
        }
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();

        // `g`/Home parks the offset at usize::MAX; the render must clamp
        // it back to the real content height…
        state.scroll_to_top();
        terminal.draw(|f| render(f, f.area(), &mut state)).unwrap();
        let max_y = state.scroll_offset;
        assert!(
            max_y > 0 && max_y < usize::MAX,
            "offset clamped to content height, got {max_y}"
        );

        // …so scroll-down actually moves again (the old bug: the offset
        // sat ~2^64 rows above the top and every down-tick was dead).
        state.scroll_down(5);
        assert_eq!(state.scroll_offset, max_y - 5);
    }

    #[test]
    fn session_title_tags_non_default_backends() {
        let mut session = mk_session("s1");
        session.provider_id = Some("pi".into());
        let title: String = session_title(&session)
            .spans
            .iter()
            .map(|sp| sp.content.clone().into_owned())
            .collect();
        assert!(title.contains("· pi"), "{title}");

        // The default backend stays untagged — chips are for the exceptions.
        let mut session = mk_session("s2");
        session.provider_id = Some("claude".into());
        let title: String = session_title(&session)
            .spans
            .iter()
            .map(|sp| sp.content.clone().into_owned())
            .collect();
        assert!(!title.contains("claude"), "{title}");
    }
}
