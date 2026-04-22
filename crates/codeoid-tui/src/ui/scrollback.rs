//! Transcript viewport. Renders every message for the focused session
//! with role-aware styling, right-aligned timestamps, and a live
//! "Thinking…" placeholder for the in-flight assistant message.

use codeoid_protocol::{MessageRole, SessionInfo, SessionMessage};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::render::{render_markdown_block, render_tool_block};
use crate::state::{AppState, Focus};

const BODY_INDENT: &str = "  ";

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let focused_pane = state.focus == Focus::Scrollback;
    let border_style = if focused_pane {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let Some(session) = state.sessions.focused() else {
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

    let msgs = state.messages.messages(&session.id);
    let mut lines: Vec<Line<'_>> = Vec::with_capacity(msgs.len() * 4);
    for m in msgs {
        let rendered = render_message(m, state.anim_tick);
        if rendered.is_empty() {
            // Placeholder messages (empty assistant/thinking mid-stream)
            // don't render in the transcript — the worker row above the
            // prompt is the single source of "something is happening".
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

    // Auto-stick: `scroll_offset` = lines above the natural bottom.
    let total_lines = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let viewport = area.height.saturating_sub(2);
    let max_top = total_lines.saturating_sub(viewport);
    let top_line = max_top.saturating_sub(state.scroll_offset);

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((top_line, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(session_title(session)),
        );

    frame.render_widget(paragraph, area);
}

fn session_title(session: &SessionInfo) -> Line<'static> {
    let mode = session.mode.map_or("interactive", |m| match m {
        codeoid_protocol::SessionMode::Interactive => "interactive",
        codeoid_protocol::SessionMode::AutoAllow => "auto-allow",
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
fn render_message(m: &SessionMessage, anim_tick: u64) -> Vec<Line<'static>> {
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
                // The tool card itself shows live phase/progress inline.
                // It's NOT redundant with the worker row — the worker
                // row is session-level ("Claude is thinking") while the
                // tool card is per-tool ("this specific Bash is still
                // running"). A long turn may have 3 completed tools and
                // 1 running — you need to see which.
                out.extend(render_tool_block(tool, anim_tick, BODY_INDENT));
            }
            if !m.content.is_empty() {
                out.extend(render_markdown_block(&m.content, BODY_INDENT));
            }
        }
        MessageRole::ToolResult => {
            for line in m.content.lines() {
                out.push(Line::from(vec![
                    Span::raw(BODY_INDENT.to_string()),
                    Span::styled("▌ ", Style::default().fg(Color::Magenta)),
                    Span::styled(line.to_string(), Style::default().fg(Color::Gray)),
                ]));
            }
        }
        MessageRole::Thinking => {
            for raw in m.content.lines() {
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
            for raw in m.content.lines() {
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
            for raw in m.content.lines() {
                out.push(Line::from(vec![
                    Span::raw(BODY_INDENT.to_string()),
                    Span::styled(raw.to_string(), Style::default().fg(Color::White)),
                ]));
            }
        }
        MessageRole::Assistant => {
            out.extend(render_markdown_block(&m.content, BODY_INDENT));
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
        MessageRole::ToolResult => (
            "tool output",
            Style::default().fg(Color::Magenta),
        ),
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
        Span::styled(
            format!("   {time}"),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

/// Render an ISO-8601 / RFC-3339 timestamp as HH:MM:SS. Best-effort; falls
/// back to the trailing 8 chars if parsing fails.
fn fmt_timestamp(raw: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt.format("%H:%M:%S").to_string(),
        Err(_) => raw.chars().rev().take(8).collect::<String>().chars().rev().collect(),
    }
}

fn short_sub(sub: &str) -> String {
    // spiffe://…/agent/<name> → <name>, else tail path segment.
    sub.rsplit('/').next().unwrap_or(sub).to_string()
}
