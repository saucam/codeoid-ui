//! Prompt editor block.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::state::{AppState, Focus};

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let focused = state.focus == Focus::Prompt;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let hint = if state.prompt_buffer.is_empty() && !focused {
        Some(Line::from(Span::styled(
            "Press [i] or [Tab] to start typing. Shift+Enter inserts a newline.",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )))
    } else {
        None
    };

    let content = match &hint {
        Some(h) => vec![h.clone()],
        None => state
            .prompt_buffer
            .split('\n')
            .map(|l| Line::from(Span::raw(l.to_string())))
            .collect(),
    };

    let title = if focused {
        " Prompt · typing "
    } else {
        " Prompt "
    };

    let p = Paragraph::new(content)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        );

    frame.render_widget(p, area);
}
