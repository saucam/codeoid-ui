//! Single-line status bar.

use codeoid_protocol::SessionMode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::state::AppState;

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut spans: Vec<Span<'_>> = Vec::new();

    if let Some(session) = state.sessions.focused() {
        spans.push(Span::styled(
            format!(" {} ", session.name),
            Style::default().add_modifier(Modifier::BOLD).bg(Color::DarkGray).fg(Color::White),
        ));
        spans.push(Span::raw(" "));
        spans.push(mode_badge(session.mode.unwrap_or(SessionMode::Interactive)));

        if let Some(usage) = &session.usage {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("${:.3}", usage.total_cost_usd),
                Style::default().fg(Color::Green),
            ));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!(
                    "{}k in / {}k out",
                    usage.input_tokens / 1000,
                    usage.output_tokens / 1000
                ),
                Style::default().fg(Color::DarkGray),
            ));
        }
    } else {
        spans.push(Span::styled(
            " No session ",
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Right-aligned help hint.
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        if let Some(err) = &state.last_error {
            format!("⚠ {err}")
        } else {
            "? help · q quit".to_string()
        },
        Style::default().fg(if state.last_error.is_some() {
            Color::Red
        } else {
            Color::DarkGray
        }),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn mode_badge(mode: SessionMode) -> Span<'static> {
    match mode {
        SessionMode::Interactive => Span::styled(
            " interactive ",
            Style::default().bg(Color::Blue).fg(Color::White),
        ),
        SessionMode::AutoAllow => Span::styled(
            " auto-allow ",
            Style::default().bg(Color::Yellow).fg(Color::Black),
        ),
        SessionMode::Autonomous => Span::styled(
            " autonomous ",
            Style::default().bg(Color::Red).fg(Color::White),
        ),
    }
}
