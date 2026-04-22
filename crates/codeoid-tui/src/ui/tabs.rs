//! Session tabs — thin horizontal bar across the top.

use codeoid_protocol::SessionStatus;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Tabs};
use ratatui::Frame;

use crate::state::AppState;

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let items: Vec<Line<'_>> = state
        .sessions
        .items()
        .iter()
        .map(|s| {
            let icon = match s.status {
                SessionStatus::Idle => "○",
                SessionStatus::Working => "◐",
                SessionStatus::WaitingApproval => "!",
                SessionStatus::Error => "✕",
            };
            Line::from(vec![
                Span::styled(format!(" {icon} "), status_style(s.status)),
                Span::raw(s.name.clone()),
            ])
        })
        .collect();

    let tabs = Tabs::new(items)
        .block(Block::default().borders(Borders::ALL).title("Sessions"))
        .select(state.sessions.focused_index().unwrap_or(0))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        );

    frame.render_widget(tabs, area);
}

fn status_style(status: SessionStatus) -> Style {
    match status {
        SessionStatus::Idle => Style::default().fg(Color::DarkGray),
        SessionStatus::Working => Style::default().fg(Color::Yellow),
        SessionStatus::WaitingApproval => Style::default().fg(Color::Magenta),
        SessionStatus::Error => Style::default().fg(Color::Red),
    }
}
