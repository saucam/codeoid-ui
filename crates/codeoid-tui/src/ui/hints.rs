//! Always-visible footer with keybinding hints. Changes based on focus
//! and current state so users discover features without the `?` modal.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::state::{AppState, Focus};

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let hints = if state.modal.is_some() {
        vec![("Esc", "dismiss"), ("?", "help"), ("Ctrl+C", "quit")]
    } else if state.focus == Focus::Prompt {
        vec![
            ("Enter", "send"),
            ("⇧Enter", "newline"),
            ("/new", "session"),
            ("PgUp", "scroll"),
            ("Ctrl+X", "interrupt"),
            ("Alt+Y/D", "approve/deny"),
            ("Esc", "blur"),
        ]
    } else {
        vec![
            ("Tab/i", "prompt"),
            ("←→", "session"),
            ("↑↓ PgUp", "scroll"),
            ("y/d", "approve/deny"),
            ("Ctrl+X", "interrupt"),
            ("?", "help"),
            ("q", "quit"),
        ]
    };

    let mut spans: Vec<Span<'_>> = Vec::with_capacity(hints.len() * 4);
    for (i, (key, desc)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                " · ",
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            (*desc).to_string(),
            Style::default().fg(Color::Gray),
        ));
    }

    // Tail: error or clean hint.
    if let Some(err) = &state.last_error {
        spans.push(Span::raw("    "));
        spans.push(Span::styled(
            format!("⚠ {err}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }

    let bg = Style::default().bg(Color::Rgb(20, 22, 28));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(bg), area);
}
