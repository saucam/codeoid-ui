//! Modal overlays — help, confirmations, protocol-drift warning.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use codeoid_protocol::UiRequestMethod;

use crate::state::{
    AppState, AskUserQuestionModal, CapabilitiesModal, CapabilitiesTab, Modal, SettingsModal,
    UiDialogModal,
};

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    // Signature matches the scope needed here; the rest of the tree can
    // borrow mutably without hitting this widget.
    let Some(modal) = &state.modal else { return };

    // Capabilities and AskUserQuestion deserve more screen real estate
    // for entries / question lists.
    let area = match modal {
        Modal::Settings(_) => centered(frame.area(), 82, 82),
        Modal::Capabilities(_) | Modal::AskUserQuestion(_) => centered(frame.area(), 80, 75),
        _ => centered(frame.area(), 60, 50),
    };

    frame.render_widget(Clear, area);

    match modal {
        Modal::Help => render_help(frame, area),
        Modal::ConfirmDestroy { name, .. } => render_confirm_destroy(frame, area, name),
        Modal::Capabilities(c) => render_capabilities(frame, area, c),
        Modal::AskUserQuestion(m) => render_ask_user_question(frame, area, m),
        Modal::UiDialog(m) => render_ui_dialog(frame, area, m),
        Modal::Settings(m) => render_settings(frame, area, m),
    }
}

/// The comprehensive settings screen — tab pills + the active tab's grouped
/// fields with an inline control per `kind`, staged-edit markers, provenance,
/// and (when a text field is active) an edit buffer.
#[allow(clippy::too_many_lines)]
fn render_settings(frame: &mut Frame<'_>, area: Rect, m: &SettingsModal) {
    let mut rows: Vec<Line<'static>> = Vec::new();

    if m.loading && m.manifest.is_none() {
        rows.push(Line::from(Span::styled(
            "loading…",
            Style::default().fg(Color::DarkGray),
        )));
    }
    if let Some(err) = &m.error {
        rows.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::Red),
        )));
    }

    if let Some(manifest) = &m.manifest {
        // Tab rail.
        let mut pills: Vec<Span<'static>> = Vec::new();
        for (i, t) in manifest.tabs.iter().enumerate() {
            let active = i == m.tab;
            let label = match &t.icon {
                Some(icon) => format!(" {icon} {} ", t.title),
                None => format!(" {} ", t.title),
            };
            let style = if active {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            pills.push(Span::styled(label, style));
            pills.push(Span::raw(" "));
        }
        rows.push(Line::from(pills));
        rows.push(Line::raw(""));

        if let Some(tab) = manifest.tabs.get(m.tab) {
            if let Some(desc) = &tab.description {
                rows.push(Line::from(Span::styled(
                    desc.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
                rows.push(Line::raw(""));
            }
            // Walk groups → visible fields, tracking a running visible index
            // so the cursor lines up with `m.selected` / `tab_fields()`.
            let mut idx = 0usize;
            for group in &tab.groups {
                let visible: Vec<&codeoid_protocol::SettingField> = group
                    .fields
                    .iter()
                    .filter(|f| m.show_advanced || !f.advanced)
                    .collect();
                if visible.is_empty() {
                    continue;
                }
                rows.push(Line::from(Span::styled(
                    group.title.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )));
                for f in visible {
                    let selected = idx == m.selected;
                    rows.push(settings_field_line(m, f, selected));
                    if selected {
                        if let Some(editing_key) = &m.editing {
                            if editing_key == &f.key {
                                rows.push(settings_edit_line(m));
                            }
                        } else if !f.help.is_empty() {
                            rows.push(Line::from(Span::styled(
                                format!("      {}", f.help),
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                    }
                    idx += 1;
                }
                rows.push(Line::raw(""));
            }
        }
    }

    // Footer: dirty count + status + backing file paths + hints.
    let dirty = m.dirty.len();
    let mut footer: Vec<Span<'static>> = Vec::new();
    if dirty > 0 {
        footer.push(Span::styled(
            format!("{dirty} unsaved "),
            Style::default().fg(Color::Cyan),
        ));
    }
    if m.restart_required {
        footer.push(Span::styled(
            "· restart to apply ",
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(status) = &m.status {
        footer.push(Span::styled(
            format!("· {status} "),
            Style::default().fg(Color::Green),
        ));
    }
    if !footer.is_empty() {
        rows.push(Line::from(footer));
    }
    if let Some(snap) = &m.snapshot {
        rows.push(Line::from(Span::styled(
            format!("files: {} · {}", snap.config_path, snap.env_path),
            Style::default().fg(Color::DarkGray),
        )));
    }
    rows.push(hint_line(
        "↑↓ field · ←→ tab · Enter toggle/edit · s save · a advanced · x clear · r refresh · Esc close",
    ));

    let block = Block::default()
        .title(" Settings ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(
        Paragraph::new(rows)
            .block(block)
            .wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

/// One field row: cursor + label + its current control state + badges.
fn settings_field_line(
    m: &SettingsModal,
    f: &codeoid_protocol::SettingField,
    selected: bool,
) -> Line<'static> {
    let cursor = if selected { "▶ " } else { "  " };
    let name_style = if selected {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let mut spans = vec![
        Span::styled(cursor.to_string(), Style::default().fg(Color::Cyan)),
        Span::styled(format!("{}: ", f.label), name_style),
        Span::styled(
            settings_value_display(m, f),
            Style::default().fg(Color::Cyan),
        ),
    ];
    if m.dirty.contains_key(&f.key) {
        spans.push(Span::styled(
            "  ●edited",
            Style::default().fg(Color::Yellow),
        ));
    }
    // Provenance (non-secret) or secret status source.
    let meta = if f.secret {
        m.snapshot
            .as_ref()
            .and_then(|s| s.secrets.get(&f.key))
            .map(|st| format!("[{}]", st.source))
    } else {
        m.snapshot
            .as_ref()
            .and_then(|s| s.values.get(&f.key))
            .map(|st| format!("[{}]", st.source))
    };
    if let Some(meta) = meta {
        spans.push(Span::styled(
            format!("  {meta}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if f.applies == "restart" {
        spans.push(Span::styled(
            "  ⟳restart",
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

/// The active edit buffer line (with a block cursor), shown under the field.
fn settings_edit_line(m: &SettingsModal) -> Line<'static> {
    Line::from(vec![
        Span::raw("      "),
        Span::styled(m.buffer.clone(), Style::default().fg(Color::White)),
        Span::styled("█", Style::default().fg(Color::Cyan)),
        Span::styled(
            "  (Enter save · Esc cancel)",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
    ])
}

/// The display string for a field's current (staged-or-effective) value.
fn settings_value_display(m: &SettingsModal, f: &codeoid_protocol::SettingField) -> String {
    if f.secret {
        let set = m
            .snapshot
            .as_ref()
            .and_then(|s| s.secrets.get(&f.key))
            .is_some_and(|st| st.set);
        let staged = m.dirty.get(&f.key);
        return match staged {
            Some(serde_json::Value::Null) => "· will clear".to_string(),
            Some(_) => "•••••••• (will update)".to_string(),
            None if set => "•••••••• (set)".to_string(),
            None => "not set".to_string(),
        };
    }
    let v = m.effective(&f.key);
    match &v {
        serde_json::Value::Null => "—".to_string(),
        serde_json::Value::Bool(b) => {
            if *b {
                "on".to_string()
            } else {
                "off".to_string()
            }
        }
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => {
            let items: Vec<String> = a
                .iter()
                .filter_map(|x| x.as_str().map(ToString::to_string))
                .collect();
            if items.is_empty() {
                "—".to_string()
            } else {
                items.join(", ")
            }
        }
        other => other.to_string(),
    }
}

/// Provider-initiated dialog (`session.ui_request`) — select / confirm /
/// input / editor. The daemon enforces the timeout; the header countdown is
/// display only.
fn render_ui_dialog(frame: &mut Frame<'_>, area: Rect, m: &UiDialogModal) {
    let req = &m.request;
    let mut rows: Vec<Line<'static>> = Vec::new();

    if let Some(message) = &req.message {
        rows.push(Line::from(Span::raw(message.clone())));
        rows.push(Line::raw(""));
    }

    match req.method {
        UiRequestMethod::Select => {
            for (i, option) in req.options.iter().flatten().enumerate() {
                let is_sel = i == m.selected;
                let cursor = if is_sel { "▶ " } else { "  " };
                let style = if is_sel {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                rows.push(Line::from(vec![
                    Span::styled(cursor.to_string(), style),
                    Span::styled(format!("{}. ", i + 1), Style::default().fg(Color::DarkGray)),
                    Span::styled(option.clone(), style),
                ]));
            }
            rows.push(Line::raw(""));
            rows.push(hint_line("↑↓ / 1-9 choose · Enter submit · Esc dismiss"));
        }
        UiRequestMethod::Confirm => {
            rows.push(hint_line("y yes · n no · Esc dismiss"));
        }
        UiRequestMethod::Input | UiRequestMethod::Editor => {
            if let Some(placeholder) = &req.placeholder {
                if m.buffer.is_empty() {
                    rows.push(Line::from(Span::styled(
                        placeholder.clone(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            // Render the buffer with a block cursor at the end. Editor
            // prefills can be multi-line; split so each line renders.
            for (i, line) in m.buffer.split('\n').enumerate() {
                let is_last = i == m.buffer.split('\n').count() - 1;
                let mut spans = vec![Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::White),
                )];
                if is_last {
                    spans.push(Span::styled("█", Style::default().fg(Color::Cyan)));
                }
                rows.push(Line::from(spans));
            }
            rows.push(Line::raw(""));
            rows.push(hint_line("type to edit · Enter submit · Esc dismiss"));
        }
    }

    let mut title = format!(" {} ", req.title);
    if let Some(timeout_ms) = req.timeout_ms {
        title = format!(" {} · auto-cancels in {}s ", req.title, timeout_ms / 1000);
    }
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(
        Paragraph::new(rows)
            .block(block)
            .wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

fn hint_line(text: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ))
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
        bind("Esc / Ctrl+X / .", "interrupt running turn"),
        bind("m", "cycle execution mode"),
        Line::raw(""),
        heading("Ask-user-question"),
        bind("y (when asked)", "open question form modal"),
        bind("1-9", "toggle option for current question"),
        bind("Tab / j k", "next / prev question"),
        bind("Enter", "submit answers"),
        bind("Esc", "cancel (sends deny back to Claude)"),
        Line::raw(""),
        heading("Tool output"),
        bind("v", "toggle full tool output (global)"),
        bind("[ / ]", "select prev / next tool block"),
        bind("Enter (in transcript)", "expand / collapse selected block"),
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
    let p = Paragraph::new(body).alignment(Alignment::Center).block(
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
        tab_pill(
            "Agents",
            matches!(c.tab, CapabilitiesTab::Agents),
            c.agents.len(),
        ),
        Span::raw("  "),
        tab_pill(
            "Skills",
            matches!(c.tab, CapabilitiesTab::Skills),
            c.skills.len(),
        ),
        Span::raw("  "),
        tab_pill(
            "MCP",
            matches!(c.tab, CapabilitiesTab::Mcp),
            c.mcp_servers.len(),
        ),
        Span::raw("  "),
        tab_pill(
            "Hooks",
            matches!(c.tab, CapabilitiesTab::Hooks),
            c.hooks.len(),
        ),
    ]));
    if let Some(workdir) = &c.workdir {
        rows.push(Line::from(vec![
            Span::styled("workdir ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                workdir.clone(),
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC),
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
                                    Span::styled("tools: ", Style::default().fg(Color::DarkGray)),
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
                                        format!("header keys (redacted): {}", headers.join(", ")),
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
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
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
        Span::styled(
            p.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
    ])
}

fn empty_hint(text: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            text,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
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
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn bind(keys: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{keys:<22}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(description),
    ])
}

fn render_ask_user_question(frame: &mut Frame<'_>, area: Rect, m: &AskUserQuestionModal) {
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.push(Line::from(vec![
        Span::styled(
            "Claude is asking ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "({} question{})",
                m.questions.len(),
                if m.questions.len() == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    rows.push(Line::raw(""));

    for (qi, q) in m.questions.iter().enumerate() {
        let is_focused_q = qi == m.focused_question;
        let prefix = if is_focused_q { "▶ " } else { "  " };
        let q_style = if is_focused_q {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        };
        let multi_hint = if q.multi_select {
            " (pick one or more)"
        } else {
            ""
        };
        let mut header_spans: Vec<Span<'static>> = vec![Span::styled(prefix.to_string(), q_style)];
        if let Some(hdr) = &q.header {
            header_spans.push(Span::styled(
                format!("[{}] ", hdr),
                Style::default().fg(Color::Magenta),
            ));
        }
        header_spans.push(Span::styled(
            format!("Q{}: {}", qi + 1, q.question),
            q_style,
        ));
        header_spans.push(Span::styled(
            multi_hint.to_string(),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));
        rows.push(Line::from(header_spans));

        for (oi, opt) in q.options.iter().enumerate() {
            let selected = q.selected.contains(&oi);
            let marker = if q.multi_select {
                if selected {
                    "[x]"
                } else {
                    "[ ]"
                }
            } else if selected {
                "(*)"
            } else {
                "( )"
            };
            let style = if selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Gray)
            };
            let label_spans = vec![
                Span::raw("    "),
                Span::styled(format!("{}. ", oi + 1), Style::default().fg(Color::Yellow)),
                Span::styled(marker.to_string(), style),
                Span::raw(" "),
                Span::styled(opt.label.clone(), style),
            ];
            rows.push(Line::from(label_spans));
            if let Some(desc) = &opt.description {
                rows.push(Line::from(vec![
                    Span::raw("        "),
                    Span::styled(
                        desc.clone(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        }
        rows.push(Line::raw(""));
    }

    rows.push(Line::raw(""));
    let submit_style = if m.all_answered() {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    rows.push(Line::from(vec![
        Span::styled("[1-9]", Style::default().fg(Color::Yellow)),
        Span::raw(" toggle option · "),
        Span::styled("[Tab/j/k]", Style::default().fg(Color::Yellow)),
        Span::raw(" next/prev question · "),
        Span::styled("[Enter]", submit_style),
        Span::raw(" submit · "),
        Span::styled("[Esc]", Style::default().fg(Color::Red)),
        Span::raw(" cancel"),
    ]));

    let title = if m.all_answered() {
        " AskUserQuestion · ready to submit "
    } else {
        " AskUserQuestion · pick an option for every question "
    };
    let p = Paragraph::new(rows).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(p, area);
}

#[cfg(test)]
mod tests {
    use codeoid_protocol::{
        AuthOkMsg, IdentityType, MessageIdentity, SessionUiRequestMsg, UiRequestMethod,
    };
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;
    use ratatui::Terminal;

    use crate::state::{AppState, Modal, UiDialogModal};

    fn mk_state() -> AppState {
        AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "spiffe://x".into(),
                name: Some("Me".into()),
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
            providers: None,
        })
    }

    fn mk_request(method: UiRequestMethod) -> SessionUiRequestMsg {
        SessionUiRequestMsg {
            session_id: "s1".into(),
            request_id: "u1".into(),
            method,
            title: "Extension asks".into(),
            message: Some("Please decide.".into()),
            options: Some(vec!["Allow".into(), "Block".into()]),
            placeholder: Some("type here".into()),
            prefill: None,
            timeout_ms: None,
            timestamp: "t".into(),
        }
    }

    fn render_to_text(state: &mut AppState) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| super::render(f, state)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(Cell::symbol)
            .collect()
    }

    #[test]
    fn select_dialog_renders_options_cursor_and_hints() {
        let mut state = mk_state();
        let mut modal = UiDialogModal::new(mk_request(UiRequestMethod::Select));
        modal.selected = 1;
        state.modal = Some(Modal::UiDialog(modal));
        let text = render_to_text(&mut state);
        assert!(text.contains("Extension asks"), "{text}");
        assert!(text.contains("Please decide."), "{text}");
        assert!(text.contains("1. Allow"), "{text}");
        assert!(text.contains("▶ 2. Block"), "{text}");
        assert!(text.contains("Enter submit"), "{text}");
    }

    #[test]
    fn confirm_dialog_renders_yn_hints_and_countdown_title() {
        let mut state = mk_state();
        let mut req = mk_request(UiRequestMethod::Confirm);
        req.timeout_ms = Some(30_000);
        state.modal = Some(Modal::UiDialog(UiDialogModal::new(req)));
        let text = render_to_text(&mut state);
        assert!(text.contains("y yes"), "{text}");
        assert!(text.contains("auto-cancels in 30s"), "{text}");
    }

    #[test]
    fn input_dialog_shows_placeholder_until_typed() {
        let mut state = mk_state();
        state.modal = Some(Modal::UiDialog(UiDialogModal::new(mk_request(
            UiRequestMethod::Input,
        ))));
        let text = render_to_text(&mut state);
        assert!(text.contains("type here"), "{text}");
        assert!(text.contains("Enter submit"), "{text}");
    }

    #[test]
    fn editor_dialog_renders_multiline_prefill_with_cursor() {
        let mut state = mk_state();
        let mut req = mk_request(UiRequestMethod::Editor);
        req.prefill = Some("line one\nline two".into());
        req.placeholder = None;
        state.modal = Some(Modal::UiDialog(UiDialogModal::new(req)));
        let text = render_to_text(&mut state);
        assert!(text.contains("line one"), "{text}");
        assert!(text.contains("line two█"), "{text}");
    }
}
