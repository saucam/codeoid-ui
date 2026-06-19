//! Top bar — connection pill on the left, session tabs in the middle,
//! usage summary on the right. Single row. Claude-code-feel.

use codeoid_protocol::{SessionMode, SessionStatus};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Tabs};
use ratatui::Frame;

use crate::render::SpinnerFrame;
use crate::state::{AppState, ConnectionState};

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(22), // connection pill
            Constraint::Min(10),    // session tabs (flex)
            Constraint::Length(30), // usage summary
        ])
        .split(area);

    render_connection_pill(frame, cols[0], &state.connection, state.anim_tick);
    render_tabs(frame, cols[1], state);
    render_usage(frame, cols[2], state);
}

fn render_connection_pill(frame: &mut Frame<'_>, area: Rect, conn: &ConnectionState, tick: u64) {
    let content = match conn {
        ConnectionState::Connected => Line::from(Span::styled(
            "● connected",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        ConnectionState::Reconnecting {
            attempt,
            next_attempt_in_secs,
        } => {
            let spinner = SpinnerFrame::for_tick(tick).glyph();
            Line::from(vec![
                Span::styled(format!("{spinner} "), Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!("reconnecting ({attempt}/5)·{next_attempt_in_secs}s"),
                    Style::default().fg(Color::Yellow),
                ),
            ])
        }
        ConnectionState::Failed { .. } => Line::from(Span::styled(
            "✕ disconnected",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
    };

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " Codeoid ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(ratatui::widgets::Paragraph::new(content), inner);
}

fn render_tabs(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let items: Vec<Line<'_>> = state
        .sessions
        .items()
        .iter()
        .map(|s| {
            let icon = match s.status {
                SessionStatus::Idle => "○",
                SessionStatus::Working => "●",
                SessionStatus::WaitingApproval => "!",
                SessionStatus::Error => "✕",
            };
            let mode_tag = match s.mode.unwrap_or(SessionMode::Interactive) {
                SessionMode::Interactive => "",
                SessionMode::Guarded => " ·guarded",
                SessionMode::Autonomous => " ·autonomous",
            };
            Line::from(vec![
                Span::styled(format!("{icon} "), status_color(s.status)),
                Span::raw(s.name.clone()),
                Span::styled(mode_tag.to_string(), Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let title = format!(" Sessions ({}) ", state.sessions.items().len());
    let block = Block::default().borders(Borders::ALL).title(title);

    if items.is_empty() {
        let placeholder = ratatui::widgets::Paragraph::new(Line::from(Span::styled(
            "no sessions — create one with `codeoid new <name> <workdir>`",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )))
        .block(block);
        frame.render_widget(placeholder, area);
        return;
    }

    let tabs = Tabs::new(items)
        .block(block)
        .select(state.sessions.focused_index().unwrap_or(0))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
        .divider(Span::styled("│", Style::default().fg(Color::DarkGray)));

    frame.render_widget(tabs, area);
}

fn render_usage(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let block = Block::default().borders(Borders::ALL).title(" Usage ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(session) = state.sessions.focused() else {
        let empty = ratatui::widgets::Paragraph::new(Line::from(Span::styled(
            "—",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(empty, inner);
        return;
    };

    let content = if let Some(usage) = &session.usage {
        let cost = format!("${:.3}", usage.total_cost_usd);
        let model = session.model.as_deref().unwrap_or("—");
        Line::from(vec![
            Span::styled(
                cost,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{}k↑ {}k↓",
                    usage.input_tokens / 1000,
                    usage.output_tokens / 1000
                ),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("  "),
            Span::styled(model.to_string(), Style::default().fg(Color::Cyan)),
        ])
    } else {
        Line::from(Span::styled(
            "no usage yet",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ))
    };

    frame.render_widget(
        ratatui::widgets::Paragraph::new(content).alignment(ratatui::layout::Alignment::Right),
        inner,
    );
}

fn status_color(status: SessionStatus) -> Style {
    match status {
        SessionStatus::Idle => Style::default().fg(Color::DarkGray),
        SessionStatus::Working => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        SessionStatus::WaitingApproval => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        SessionStatus::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}
