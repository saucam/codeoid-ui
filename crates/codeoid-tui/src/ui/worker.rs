//! Single-line working indicator — sits between transcript and prompt.
//!
//! Always-on; either shows "what Claude is doing right now" or a subtle
//! hint about how to scroll. Treating this as a dedicated row (instead of
//! wedging it into the status bar) keeps the signal stable and visually
//! grounded in the conversation pane.

use codeoid_protocol::{SessionInfo, SessionMessage, SessionStatus, ToolState};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::commands::{self, CATALOG};
use crate::render::spinner::seed_from;
use crate::render::{verb_phrase, SpinnerFrame};
use crate::state::AppState;

/// How long (in 100 ms ticks) we keep showing "thinking" after the last
/// delta, used when the daemon hasn't flipped status to Working yet.
const ACTIVITY_FALLBACK_TICKS: u64 = 20;

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    // Command-mode takes precedence — swap the entire row for a matching
    // palette so the user can see what's possible without leaving the
    // prompt.
    if state.is_command_mode() {
        let palette = build_palette(state);
        frame.render_widget(Paragraph::new(palette).alignment(Alignment::Left), area);
        return;
    }

    let line = build_line(state).unwrap_or_else(|| idle_line(state));

    // Right side: a subtle scroll position hint.
    let right = scroll_hint(state);

    // Two paragraphs, left-aligned and right-aligned, painted on the same row.
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Left), area);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
}

fn build_palette(state: &AppState) -> Line<'static> {
    let query = state.command_query().unwrap_or("");
    // Built-ins first, then the focused session's provider commands
    // (pi extensions, prompt templates, skills) — same prefix filter.
    let mut matches: Vec<(String, String)> = commands::filter_catalog(query)
        .into_iter()
        .map(|(usage, desc)| ((*usage).to_string(), (*desc).to_string()))
        .collect();
    let query_lower = query.to_ascii_lowercase();
    for cmd in state.focused_provider_commands() {
        if !cmd.name.to_ascii_lowercase().starts_with(&query_lower) {
            continue;
        }
        let usage = match &cmd.argument_hint {
            Some(hint) => format!("/{} {hint}", cmd.name),
            None => format!("/{}", cmd.name),
        };
        let desc = match (&cmd.description, &cmd.source) {
            (Some(d), Some(s)) => format!("{d} ({s})"),
            (Some(d), None) => d.clone(),
            (None, Some(s)) => format!("provider command ({s})"),
            (None, None) => "provider command".to_string(),
        };
        matches.push((usage, desc));
    }

    if matches.is_empty() {
        return Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "⌘ no matching command",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "— try /help for the full list",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]);
    }

    // Cap to 4 visible matches so narrow terminals don't clip painfully.
    let display_limit = 4;
    let total = matches.len();
    let visible = matches.iter().take(display_limit);

    let mut spans: Vec<Span<'_>> = vec![
        Span::raw("  "),
        Span::styled(
            "⌘ ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    for (i, (usage, desc)) in visible.enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            usage.clone(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("— {desc}"),
            Style::default().fg(Color::Gray),
        ));
    }
    if total > display_limit {
        spans.push(Span::styled(
            format!("  (+{} more)", total - display_limit),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));
    }
    if total > 1 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("[Tab] picks",),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));
    } else if total == 1 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "[Tab] autocomplete",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));
    }

    // Silence an unused-import warning when no palette-matches path needs
    // CATALOG: we read it via commands::filter_catalog and commands::CATALOG
    // is only publicly re-exported for tests / future palette pages.
    let _ = CATALOG;

    Line::from(spans)
}

fn build_line(state: &AppState) -> Option<Line<'static>> {
    let session = state.sessions.focused()?;
    let msgs = state.messages.messages(&session.id);

    // Tool in flight — most specific signal.
    if let Some(l) = tool_line(msgs, state.anim_tick) {
        return Some(l);
    }

    // Pending approval is now carried by the dedicated high-visibility
    // approval banner (see `ui::approval`), so the worker row no longer
    // duplicates it.

    // Session-level signal: daemon tells us it's working. Trusted.
    if matches!(session.status, SessionStatus::Working) {
        return Some(thinking_line(session, state.anim_tick));
    }

    // Fallback: recent delta activity for THIS session, even if status
    // hasn't flipped yet. Per-session so an active session elsewhere can't
    // trick us into animating an idle one.
    if let Some(ticks_since) = state.ticks_since_activity(&session.id) {
        if ticks_since <= ACTIVITY_FALLBACK_TICKS {
            return Some(thinking_line(session, state.anim_tick));
        }
    }

    None
}

/// Idle row — quiet "idle" plus the focused session's model when known,
/// so the active model is always visible at a glance (matches the web's
/// model indicator).
fn idle_line(state: &AppState) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "  idle",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )];
    if let Some(model) = state.sessions.focused().and_then(|s| s.model.as_deref()) {
        let disp = state.model_display(model);
        spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("model: {disp}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));
    }
    Line::from(spans)
}

fn thinking_line(session: &SessionInfo, tick: u64) -> Line<'static> {
    let spinner = SpinnerFrame::for_tick(tick).glyph();
    let verb = verb_phrase(seed_from(&session.id), tick);
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            spinner.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{verb}…"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC),
        ),
        Span::raw("  "),
        Span::styled("[Esc] interrupt", Style::default().fg(Color::DarkGray)),
    ])
}

fn tool_line(msgs: &[SessionMessage], tick: u64) -> Option<Line<'static>> {
    let tool = msgs.iter().rev().find_map(|m| {
        let t = m.tool.as_ref()?;
        match &t.state {
            ToolState::Streaming { .. } | ToolState::Executing { .. } => Some(t),
            _ => None,
        }
    })?;

    let spinner = SpinnerFrame::for_tick(tick).glyph();
    let phrase = match &tool.state {
        ToolState::Streaming { .. } => format!("drafting {}…", tool.name),
        ToolState::Executing { elapsed_ms, .. } => match elapsed_ms {
            Some(ms) => format!("running {} · {}", tool.name, fmt_ms(*ms)),
            None => format!("running {}", tool.name),
        },
        _ => unreachable!(),
    };

    Some(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            spinner.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            phrase,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC),
        ),
        Span::raw("  "),
        Span::styled("[Esc] interrupt", Style::default().fg(Color::DarkGray)),
    ]))
}

fn scroll_hint(state: &AppState) -> Line<'static> {
    if state.scroll_offset == 0 {
        return Line::from(Span::styled(
            "↓ following · ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));
    }

    // Anchored mode. Highlight the "new below" count when content has
    // streamed in since the user scrolled — that's the actionable
    // signal ("there's something new for you"). The plain "scrolled ↑N"
    // by itself is just navigational state.
    let mut spans: Vec<Span<'static>> = Vec::new();
    if state.unseen_below_rows > 0 {
        spans.push(Span::styled(
            format!("↓ {} new", state.unseen_below_rows),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
    }
    spans.push(Span::styled(
        format!("scrolled ↑{}", state.scroll_offset),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::ITALIC),
    ));
    spans.push(Span::styled(
        "  ·  [End] catch up · ",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ));

    Line::from(spans)
}

fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{mins}m{secs:02}s")
    }
}

#[cfg(test)]
mod tests {
    use codeoid_protocol::{
        AuthOkMsg, IdentityType, MessageIdentity, ProviderCommand, SessionInfo, SessionStatus,
    };

    use super::build_palette;
    use crate::state::AppState;

    fn mk_state() -> AppState {
        let mut state = AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "spiffe://x".into(),
                name: Some("Me".into()),
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
            capabilities: None,
        });
        state.sessions.upsert(SessionInfo {
            id: "s1".into(),
            name: "demo".into(),
            workdir: "/tmp".into(),
            status: SessionStatus::Idle,
            created_by: "u".into(),
            created_at: "t".into(),
            attached_clients: 0,
            mode: None,
            turns_remaining: None,
            pinned_files: None,
            agent_uri: None,
            subagents: None,
            usage: None,
            rotation: None,
            queued_messages: None,
            model: None,
            fallback_model: None,
        });
        state.provider_commands.insert(
            "s1".into(),
            vec![
                ProviderCommand {
                    name: "review".into(),
                    description: Some("Review the diff".into()),
                    source: Some("extension".into()),
                    argument_hint: Some("<scope>".into()),
                },
                ProviderCommand {
                    name: "fix-tests".into(),
                    description: None,
                    source: None,
                    argument_hint: None,
                },
            ],
        );
        state
    }

    fn palette_text(state: &AppState) -> String {
        build_palette(state)
            .spans
            .iter()
            .map(|s| s.content.clone().into_owned())
            .collect()
    }

    #[test]
    fn palette_merges_provider_commands_after_builtins() {
        let mut state = mk_state();
        state.prompt.insert_str("/re");
        let text = palette_text(&state);
        assert!(text.contains("/rename"), "builtin first: {text}");
        assert!(
            text.contains("/review <scope>") || text.contains("+"),
            "provider command (or overflow counter) visible: {text}"
        );
    }

    #[test]
    fn palette_annotates_source_and_falls_back_without_description() {
        let mut state = mk_state();
        state.prompt.insert_str("/review");
        let text = palette_text(&state);
        assert!(text.contains("Review the diff (extension)"), "{text}");

        let mut state2 = mk_state();
        state2.prompt.insert_str("/fix-");
        let text2 = palette_text(&state2);
        assert!(text2.contains("provider command"), "{text2}");
    }

    #[test]
    fn palette_still_reports_no_match_for_unknown_verbs() {
        let mut state = mk_state();
        state.prompt.insert_str("/zzz");
        let text = palette_text(&state);
        assert!(text.contains("no matching command"), "{text}");
    }
}
