//! Prompt editor — `tui-textarea`-backed, with a distinct "command mode"
//! appearance when the first character is `/`.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::state::{AppState, Focus};

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &mut AppState) {
    let focused = state.focus == Focus::Prompt;
    let command_mode = state.is_command_mode();

    let (border_style, accent) = if command_mode {
        (
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
            Color::Magenta,
        )
    } else if focused {
        (Style::default().fg(Color::Cyan), Color::Cyan)
    } else {
        (Style::default().fg(Color::DarkGray), Color::DarkGray)
    };

    let title = build_title(focused, command_mode, state, accent);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    state.prompt.set_block(block);

    // Hide the cursor when not focused so users don't wonder where their
    // keys are going.
    if focused {
        state.prompt.set_cursor_style(
            Style::default()
                .bg(if command_mode { Color::Magenta } else { Color::Cyan })
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
    } else {
        state.prompt.set_cursor_style(Style::default());
    }

    frame.render_widget(&state.prompt, area);
}

fn build_title(
    focused: bool,
    command_mode: bool,
    state: &AppState,
    accent: Color,
) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];

    if command_mode {
        spans.push(Span::styled(
            "⌘ Command",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
        if let Some(q) = state.command_query() {
            if !q.is_empty() {
                spans.push(Span::raw(" · "));
                spans.push(Span::styled(
                    format!("/{q}"),
                    Style::default()
                        .fg(accent)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
        }
    } else {
        spans.push(Span::styled(
            if focused { "Prompt · typing" } else { "Prompt" },
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD),
        ));
        if !state.prompt_is_empty() {
            let chars: usize = state
                .prompt
                .lines()
                .iter()
                .map(|l| l.chars().count())
                .sum();
            let lines = state.prompt.lines().len();
            spans.push(Span::raw(" · "));
            spans.push(Span::styled(
                format!("{chars} chars · {lines} line(s)"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}
