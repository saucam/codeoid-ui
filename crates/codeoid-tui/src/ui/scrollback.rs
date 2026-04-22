//! Scrollback viewport. Renders all messages for the focused session.

use codeoid_protocol::{MessageRole, SessionMessage, ToolPhase};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::state::AppState;

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let Some(session) = state.sessions.focused() else {
        let placeholder = Paragraph::new("No session. Create one to get started.")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL).title("Transcript"));
        frame.render_widget(placeholder, area);
        return;
    };

    let msgs = state.messages.messages(&session.id);
    let mut lines: Vec<Line<'_>> = Vec::with_capacity(msgs.len() * 2);
    for m in msgs {
        lines.extend(render_message(m));
        lines.push(Line::raw(""));
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((state.scroll_offset, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} — {} ", session.name, format_status(session))),
        );

    frame.render_widget(paragraph, area);
}

fn format_status(session: &codeoid_protocol::SessionInfo) -> String {
    let mode = session
        .mode
        .map(|m| match m {
            codeoid_protocol::SessionMode::Interactive => "interactive",
            codeoid_protocol::SessionMode::AutoAllow => "auto-allow",
            codeoid_protocol::SessionMode::Autonomous => "autonomous",
        })
        .unwrap_or("interactive");
    format!("{:?} · {}", session.status, mode)
}

fn render_message(m: &SessionMessage) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let (role_label, role_style) = role_styling(m.role);

    let identity = m.identity.name.clone().unwrap_or_else(|| m.identity.sub.clone());
    let header = Line::from(vec![
        Span::styled(format!("{role_label} "), role_style),
        Span::styled(identity, Style::default().fg(Color::DarkGray)),
    ]);
    out.push(header);

    if let Some(tool) = &m.tool {
        out.push(Line::from(vec![
            Span::styled("  ⚡ ", Style::default().fg(Color::Yellow)),
            Span::styled(
                tool.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                phase_label(tool.state.phase()),
                phase_style(tool.state.phase()),
            ),
        ]));
    }

    if !m.content.is_empty() {
        for raw in m.content.lines() {
            out.push(Line::from(Span::raw(format!("  {raw}"))));
        }
    }

    out
}

fn role_styling(role: MessageRole) -> (&'static str, Style) {
    match role {
        MessageRole::User => ("▶ user", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        MessageRole::Assistant => (
            "◆ assistant",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        MessageRole::Thinking => ("  thinking", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
        MessageRole::ToolCall => (
            "  tool_call",
            Style::default().fg(Color::Yellow),
        ),
        MessageRole::ToolResult => ("  tool_result", Style::default().fg(Color::Magenta)),
        MessageRole::System => ("• system", Style::default().fg(Color::Red)),
        MessageRole::Info => ("  info", Style::default().fg(Color::DarkGray)),
    }
}

fn phase_label(p: ToolPhase) -> &'static str {
    match p {
        ToolPhase::Streaming => "streaming…",
        ToolPhase::WaitingConfirmation => "waiting for approval (y/d)",
        ToolPhase::Executing => "running…",
        ToolPhase::Completed => "✓ completed",
        ToolPhase::Cancelled => "✕ cancelled",
    }
}

fn phase_style(p: ToolPhase) -> Style {
    match p {
        ToolPhase::Streaming | ToolPhase::Executing => Style::default().fg(Color::Yellow),
        ToolPhase::WaitingConfirmation => {
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
        }
        ToolPhase::Completed => Style::default().fg(Color::Green),
        ToolPhase::Cancelled => Style::default().fg(Color::Red),
    }
}
