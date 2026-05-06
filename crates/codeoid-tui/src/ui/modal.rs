//! Modal overlays — help, confirmations, protocol-drift warning.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::state::{AppState, CapabilitiesModal, CapabilitiesTab, Modal};

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    // Signature matches the scope needed here; the rest of the tree can
    // borrow mutably without hitting this widget.
    let Some(modal) = &state.modal else { return };

    // Capabilities deserves more screen real estate for the entries.
    let area = match modal {
        Modal::Capabilities(_) => centered(frame.area(), 80, 75),
        _ => centered(frame.area(), 60, 50),
    };

    frame.render_widget(Clear, area);

    match modal {
        Modal::Help => render_help(frame, area),
        Modal::ConfirmDestroy { name, .. } => render_confirm_destroy(frame, area, name),
        Modal::Capabilities(c) => render_capabilities(frame, area, c),
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

fn render_capabilities(frame: &mut Frame<'_>, area: Rect, c: &CapabilitiesModal) {
    let title = match c.tab {
        CapabilitiesTab::Agents => " Capabilities — Agents ",
        CapabilitiesTab::Skills => " Capabilities — Skills ",
        CapabilitiesTab::Mcp => " Capabilities — MCP servers ",
        CapabilitiesTab::Hooks => " Capabilities — Hooks ",
    };

    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.push(Line::from(vec![
        tab_pill("Agents", matches!(c.tab, CapabilitiesTab::Agents), c.agents.len()),
        Span::raw("  "),
        tab_pill("Skills", matches!(c.tab, CapabilitiesTab::Skills), c.skills.len()),
        Span::raw("  "),
        tab_pill("MCP", matches!(c.tab, CapabilitiesTab::Mcp), c.mcp_servers.len()),
        Span::raw("  "),
        tab_pill("Hooks", matches!(c.tab, CapabilitiesTab::Hooks), c.hooks.len()),
    ]));
    if let Some(workdir) = &c.workdir {
        rows.push(Line::from(vec![
            Span::styled("workdir ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                workdir.clone(),
                Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
    rows.push(Line::raw(""));

    if c.loading {
        rows.push(Line::from(Span::styled(
            "loading…",
            Style::default().fg(Color::DarkGray),
        )));
    } else if let Some(err) = &c.error {
        rows.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::Red),
        )));
    } else {
        match c.tab {
            CapabilitiesTab::Agents => {
                if c.agents.is_empty() {
                    rows.push(empty_hint("No subagents loaded."));
                } else {
                    for a in &c.agents {
                        rows.push(item_header(&a.name, scope_label(a.scope)));
                        if let Some(d) = &a.description {
                            rows.push(item_desc(d));
                        }
                        if let Some(tools) = &a.tools {
                            if !tools.is_empty() {
                                rows.push(Line::from(vec![
                                    Span::raw("    "),
                                    Span::styled(
                                        "tools: ",
                                        Style::default().fg(Color::DarkGray),
                                    ),
                                    Span::styled(
                                        tools.join(", "),
                                        Style::default().fg(Color::Gray),
                                    ),
                                ]));
                            }
                        }
                        rows.push(item_path(&a.path));
                        rows.push(Line::raw(""));
                    }
                }
            }
            CapabilitiesTab::Skills => {
                if c.skills.is_empty() {
                    rows.push(empty_hint("No skills loaded."));
                } else {
                    for s in &c.skills {
                        rows.push(item_header(&format!("/{}", s.name), scope_label(s.scope)));
                        if let Some(d) = &s.description {
                            rows.push(item_desc(d));
                        }
                        rows.push(item_path(&s.path));
                        rows.push(Line::raw(""));
                    }
                }
            }
            CapabilitiesTab::Mcp => {
                if c.mcp_servers.is_empty() {
                    rows.push(empty_hint(
                        "No MCP servers configured. Add an `mcpServers` block to settings.json.",
                    ));
                } else {
                    for m in &c.mcp_servers {
                        rows.push(item_header(&m.name, scope_label(m.scope)));
                        if let Some(cmd) = &m.command {
                            let line = if m.args.is_empty() {
                                cmd.clone()
                            } else {
                                format!("{cmd} {}", m.args.join(" "))
                            };
                            rows.push(Line::from(vec![
                                Span::raw("    "),
                                Span::styled(line, Style::default().fg(Color::Gray)),
                            ]));
                        }
                        if let Some(url) = &m.url {
                            rows.push(Line::from(vec![
                                Span::raw("    url: "),
                                Span::styled(url.clone(), Style::default().fg(Color::Gray)),
                            ]));
                        }
                        if !m.env_keys.is_empty() {
                            rows.push(Line::from(vec![
                                Span::raw("    "),
                                Span::styled(
                                    format!("env keys (redacted): {}", m.env_keys.join(", ")),
                                    Style::default().fg(Color::DarkGray),
                                ),
                            ]));
                        }
                        if let Some(headers) = &m.header_keys {
                            if !headers.is_empty() {
                                rows.push(Line::from(vec![
                                    Span::raw("    "),
                                    Span::styled(
                                        format!(
                                            "header keys (redacted): {}",
                                            headers.join(", ")
                                        ),
                                        Style::default().fg(Color::DarkGray),
                                    ),
                                ]));
                            }
                        }
                        rows.push(item_path(&m.path));
                        rows.push(Line::raw(""));
                    }
                }
            }
            CapabilitiesTab::Hooks => {
                if c.hooks.is_empty() {
                    rows.push(empty_hint("No hooks configured."));
                } else {
                    for h in &c.hooks {
                        let mut header_line = vec![
                            Span::raw("  "),
                            Span::styled(
                                h.event.clone(),
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw("  "),
                            scope_pill(h.scope),
                        ];
                        if let Some(matcher) = &h.matcher {
                            header_line.push(Span::raw("  "));
                            header_line.push(Span::styled(
                                matcher.clone(),
                                Style::default().fg(Color::Magenta),
                            ));
                        }
                        rows.push(Line::from(header_line));
                        rows.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(h.command.clone(), Style::default().fg(Color::Gray)),
                        ]));
                        rows.push(item_path(&h.path));
                        rows.push(Line::raw(""));
                    }
                }
            }
        }
    }

    let p = Paragraph::new(rows).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_alignment(Alignment::Center),
    );
    frame.render_widget(p, area);
}

fn tab_pill(label: &'static str, active: bool, count: usize) -> Span<'static> {
    let style = if active {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Span::styled(format!(" {label} {count} "), style)
}

fn item_header(name: &str, scope: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            name.to_string(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        scope_pill_str(scope),
    ])
}

fn item_desc(desc: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw("    "),
        Span::styled(desc.to_string(), Style::default().fg(Color::Gray)),
    ])
}

fn item_path(p: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw("    "),
        Span::styled(p.to_string(), Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
    ])
}

fn empty_hint(text: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(text, Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
    ])
}

fn scope_label(scope: codeoid_protocol::ClaudeConfigScope) -> &'static str {
    match scope {
        codeoid_protocol::ClaudeConfigScope::Workdir => "ws",
        codeoid_protocol::ClaudeConfigScope::Global => "global",
    }
}

fn scope_pill(scope: codeoid_protocol::ClaudeConfigScope) -> Span<'static> {
    scope_pill_str(scope_label(scope))
}

fn scope_pill_str(label: &'static str) -> Span<'static> {
    let (fg, bg) = if label == "ws" {
        (Color::Black, Color::Cyan)
    } else {
        (Color::White, Color::DarkGray)
    };
    Span::styled(
        format!(" {label} "),
        Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    )
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
