//! Claude-code-style tool invocation rendering.
//!
//! Renders a compact "card" per tool call that evolves in place as the
//! tool state progresses. Each phase has a distinctive header icon, an
//! optional inline spinner, a summary of the tool input, and any output
//! that's already landed.

use codeoid_protocol::{ToolInfo, ToolState};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::markdown::inline_spans;
use super::spinner::{seed_from, SpinnerFrame};

pub fn render_tool_block(tool: &ToolInfo, anim_tick: u64, indent: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let spinner = SpinnerFrame::for_tick(anim_tick).glyph();

    // Header row — always present.
    let (icon, icon_style) = header_icon(&tool.state, spinner);
    out.push(Line::from(vec![
        Span::raw(indent.to_owned()),
        Span::styled(icon.to_string(), icon_style),
        Span::raw(" "),
        Span::styled(
            tool.name.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        phase_summary(tool, anim_tick),
    ]));

    // Input preview — single most informative line we can extract from the
    // tool's `input` value. Only shown once input is known (i.e. not during
    // early `streaming` without partialInput).
    if let Some(input_line) = input_preview(&tool.state) {
        out.push(Line::from(vec![
            Span::raw(indent.to_owned()),
            Span::raw("    "),
            Span::styled("› ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                input_line,
                Style::default()
                    .fg(Color::Rgb(200, 210, 220))
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
    }

    // Description (waiting_confirmation phase).
    if let ToolState::WaitingConfirmation { description, .. } = &tool.state {
        let mut spans = vec![
            Span::raw(indent.to_owned()),
            Span::raw("    "),
            Span::styled("⎯ ", Style::default().fg(Color::Magenta)),
        ];
        spans.extend(inline_spans(description, Style::default()));
        out.push(Line::from(spans));

        out.push(Line::from(vec![
            Span::raw(indent.to_owned()),
            Span::raw("    "),
            Span::styled(
                "Press ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "[y]",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " approve · ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "[d]",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" deny", Style::default().fg(Color::DarkGray)),
        ]));
    }

    // Output body (executing with progress, or completed with output).
    match &tool.state {
        ToolState::Executing {
            progress: Some(p), ..
        } => {
            out.push(output_frame_line("· ", p, Color::Yellow, indent));
        }
        ToolState::Completed {
            output: Some(output),
            success,
            ..
        } => {
            let color = if *success { Color::Green } else { Color::Red };
            for line in output.lines().take(40) {
                out.push(output_frame_line("", line, color, indent));
            }
            if output.lines().count() > 40 {
                out.push(Line::from(vec![
                    Span::raw(indent.to_owned()),
                    Span::raw("    "),
                    Span::styled(
                        format!(
                            "… {} more line(s) truncated",
                            output.lines().count().saturating_sub(40)
                        ),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        }
        ToolState::Cancelled {
            message: Some(m), ..
        } => {
            out.push(output_frame_line("", m, Color::Red, indent));
        }
        _ => {}
    }

    // Stable, non-colliding seed so sibling tool cards get different verbs
    // if we ever render both in working state simultaneously.
    let _seed = seed_from(&tool.tool_id);
    out
}

fn header_icon(state: &ToolState, spinner: &str) -> (String, Style) {
    match state {
        ToolState::Streaming { .. } => (
            spinner.to_string(),
            Style::default().fg(Color::Yellow),
        ),
        ToolState::WaitingConfirmation { .. } => (
            "⚠".to_string(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        ToolState::Executing { .. } => (
            spinner.to_string(),
            Style::default().fg(Color::Yellow),
        ),
        ToolState::Completed { success, .. } => {
            if *success {
                ("✓".to_string(), Style::default().fg(Color::Green))
            } else {
                ("✕".to_string(), Style::default().fg(Color::Red))
            }
        }
        ToolState::Cancelled { .. } => ("✕".to_string(), Style::default().fg(Color::Red)),
    }
}

fn phase_summary(tool: &ToolInfo, anim_tick: u64) -> Span<'static> {
    use super::spinner::verb_phrase;
    let seed = seed_from(&tool.tool_id);

    match &tool.state {
        ToolState::Streaming { .. } => Span::styled(
            format!("{}…", verb_phrase(seed, anim_tick)),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC),
        ),
        ToolState::WaitingConfirmation { .. } => Span::styled(
            "awaiting your approval",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        ToolState::Executing { elapsed_ms, .. } => Span::styled(
            format!("running · {}", fmt_elapsed(*elapsed_ms)),
            Style::default().fg(Color::Yellow),
        ),
        ToolState::Completed {
            success,
            elapsed_ms,
            ..
        } => {
            let tag = if *success { "done" } else { "failed" };
            Span::styled(
                format!("{tag} · {}", fmt_elapsed(*elapsed_ms)),
                Style::default().fg(if *success { Color::Green } else { Color::Red }),
            )
        }
        ToolState::Cancelled { reason, .. } => Span::styled(
            format!("cancelled ({reason:?})"),
            Style::default().fg(Color::Red),
        ),
    }
}

fn fmt_elapsed(ms: Option<u64>) -> String {
    let Some(ms) = ms else {
        return "—".to_string();
    };
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

fn output_frame_line(prefix: &'static str, body: &str, color: Color, indent: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(indent.to_owned()),
        Span::raw("    "),
        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
        Span::styled(prefix.to_string(), Style::default().fg(color)),
        Span::styled(body.to_string(), Style::default().fg(color)),
    ])
}

/// Best-effort compact preview of the tool input. Handles a handful of
/// common Anthropic tool shapes (Bash, Read, Edit, Write, Grep, Glob) —
/// falls back to a truncated JSON repr for everything else.
fn input_preview(state: &ToolState) -> Option<String> {
    let input = match state {
        ToolState::Streaming {
            partial_input: Some(v),
        }
        | ToolState::WaitingConfirmation { input: v, .. } => v,
        _ => return None,
    };

    let obj = input.as_object()?;

    // Common parameter shapes — pick the most informative single field.
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "description",
    ] {
        if let Some(Value::String(s)) = obj.get(key) {
            return Some(truncate(s, 120));
        }
    }

    // Edit/Write tools — show the file path even if there's no obvious key.
    if let Some(Value::String(path)) = obj.get("filepath").or_else(|| obj.get("filePath")) {
        return Some(truncate(path, 120));
    }

    // Fallback: short JSON dump.
    let compact = serde_json::to_string(input).ok()?;
    Some(truncate(&compact, 120))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let prefix: String = s.chars().take(max).collect();
        format!("{prefix}…")
    }
}
