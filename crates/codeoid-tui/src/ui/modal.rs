//! Modal overlays — help, confirmations, protocol-drift warning.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::state::{AppState, Modal};

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let Some(modal) = &state.modal else { return };
    let area = centered(frame.area(), 60, 50);

    frame.render_widget(Clear, area);

    match modal {
        Modal::Help => render_help(frame, area),
        Modal::ConfirmDestroy { name, .. } => render_confirm_destroy(frame, area, name),
        Modal::ProtocolDrift { client, daemon } => {
            render_protocol_drift(frame, area, *client, *daemon);
        }
    }
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn render_help(frame: &mut Frame<'_>, area: Rect) {
    let rows = vec![
        heading("Navigation"),
        bind("Tab / i", "focus prompt"),
        bind("Esc", "blur prompt"),
        bind("← →  p n", "prev / next session"),
        bind("PgUp PgDn", "scroll transcript"),
        Line::raw(""),
        heading("Actions"),
        bind("Enter", "send prompt"),
        bind("Shift+Enter / Ctrl+J", "newline"),
        bind("y", "approve pending tool"),
        bind("d", "deny pending tool"),
        bind("Ctrl+X / .", "interrupt session"),
        bind("m", "cycle execution mode"),
        Line::raw(""),
        heading("Meta"),
        bind("?", "toggle this help"),
        bind("q / Ctrl+C", "quit"),
    ];

    let p = Paragraph::new(rows).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Keybindings ")
            .title_alignment(Alignment::Center),
    );
    frame.render_widget(p, area);
}

fn render_confirm_destroy(frame: &mut Frame<'_>, area: Rect, name: &str) {
    let body = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("Destroy session “{name}”?"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from("This deletes all scrollback and backing Claude Code state."),
        Line::raw(""),
        Line::from(vec![
            Span::styled("[y] ", Style::default().fg(Color::Red)),
            Span::raw("destroy   "),
            Span::styled("[n] ", Style::default().fg(Color::Green)),
            Span::raw("cancel"),
        ]),
    ];
    let p = Paragraph::new(body)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm destroy ")
                .border_style(Style::default().fg(Color::Red)),
        );
    frame.render_widget(p, area);
}

fn render_protocol_drift(frame: &mut Frame<'_>, area: Rect, client: u32, daemon: Option<u32>) {
    let daemon_label = daemon.map_or_else(|| "unknown (pre-v1)".to_string(), |v| v.to_string());
    let body = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "⚠  Protocol version drift",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(format!("Client speaks version {client}")),
        Line::from(format!("Daemon speaks version {daemon_label}")),
        Line::raw(""),
        Line::from(Span::styled(
            "The daemon treats additions as non-breaking.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "If you see weird behaviour, upgrade one side.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::raw(""),
        Line::from("[?] dismiss"),
    ];
    let p = Paragraph::new(body)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Protocol drift ")
                .border_style(Style::default().fg(Color::Yellow)),
        );
    frame.render_widget(p, area);
}

fn heading(text: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ))
}

fn bind(keys: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{keys:<22}"),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::raw(description),
    ])
}
