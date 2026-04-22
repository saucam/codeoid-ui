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

    let line = build_line(state).unwrap_or_else(|| {
        Line::from(Span::styled(
            "  idle",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ))
    });

    // Right side: a subtle scroll position hint.
    let right = scroll_hint(state);

    // Two paragraphs, left-aligned and right-aligned, painted on the same row.
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Left), area);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
}

fn build_palette(state: &AppState) -> Line<'static> {
    let query = state.command_query().unwrap_or("");
    let matches = commands::filter_catalog(query);

    if matches.is_empty() {
        return Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "⌘ no matching command",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
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
            spans.push(Span::styled(
                "  ·  ",
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::styled(
            (*usage).to_string(),
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

    // Waiting on approval — static.
    if msgs.iter().any(|m| {
        m.tool
            .as_ref()
            .is_some_and(|t| matches!(&t.state, ToolState::WaitingConfirmation { .. }))
    }) {
        return Some(Line::from(vec![
            Span::styled(
                "  ⚠ ",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "waiting on your approval — press [y] accept or [d] deny",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }

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
        Span::styled(
            "[Ctrl+X] interrupt",
            Style::default().fg(Color::DarkGray),
        ),
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
        Span::styled(
            "[Ctrl+X] interrupt",
            Style::default().fg(Color::DarkGray),
        ),
    ]))
}

fn scroll_hint(state: &AppState) -> Line<'static> {
    if state.scroll_offset == 0 {
        Line::from(Span::styled(
            "↓ following · ",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ))
    } else {
        Line::from(Span::styled(
            format!("scrolled ↑{} · [PgDn] catch up · ", state.scroll_offset),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC),
        ))
    }
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
