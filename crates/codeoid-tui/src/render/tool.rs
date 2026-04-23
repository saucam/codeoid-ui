//! Tool invocation rendering.
//!
//! Each tool call renders inline in the transcript as:
//!
//! ```text
//!   ⠋ bash  ·  running · 2.3s
//!     › git status --short
//!     M Cargo.toml            ← colors preserved via ANSI parser
//!     ?? new_file.rs
//! ```
//!
//! A small phase-colored icon + summary up top, an italic input preview,
//! and the full command output indented beneath it. Colors in the output
//! (e.g. `git status`, `cargo check`) are preserved via the ANSI parser
//! in [`super::ansi`]. No boxes, no borders — the shape of the output
//! comes from indentation alone.

use codeoid_protocol::{ToolInfo, ToolState};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ansi::parse_ansi;
use super::markdown::inline_spans;
use super::spinner::{seed_from, SpinnerFrame};

/// Max body lines we show per tool call. Large `git log`s or `cargo
/// build` dumps still render their head; the remainder collapses to a
/// "+N more" tail so the transcript doesn't get drowned.
const MAX_BODY_LINES: usize = 40;

/// Render a tool invocation. `indent` is the margin applied to the
/// header; body content sits at `indent + "  "` so it reads as a
/// continuation.
pub fn render_tool_block(tool: &ToolInfo, anim_tick: u64, indent: &str) -> Vec<Line<'static>> {
    let spinner = SpinnerFrame::for_tick(anim_tick).glyph();
    let body_indent = format!("{indent}  ");

    let mut out: Vec<Line<'static>> = Vec::new();

    // Header: "  ⠋ bash  ·  running · 2.3s"
    let (icon, icon_style) = header_icon(&tool.state, spinner);
    let _ = seed_from(&tool.tool_id); // keep usage marker
    out.push(Line::from(vec![
        Span::raw(indent.to_owned()),
        Span::styled(icon, icon_style),
        Span::raw(" "),
        Span::styled(
            tool.name.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        phase_summary(tool, anim_tick),
    ]));

    // Input preview: compact, italic, dim. Only when we have an input.
    if let Some(preview) = input_preview(&tool.state) {
        out.push(Line::from(vec![
            Span::raw(body_indent.clone()),
            Span::styled("› ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                preview,
                Style::default()
                    .fg(Color::Rgb(200, 210, 220))
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
    }

    // Waiting-for-approval: description + key prompt.
    if let ToolState::WaitingConfirmation { description, .. } = &tool.state {
        let mut desc_spans = vec![
            Span::raw(body_indent.clone()),
            Span::styled("⎯ ", Style::default().fg(Color::Magenta)),
        ];
        desc_spans.extend(inline_spans(description, Style::default()));
        out.push(Line::from(desc_spans));
        out.push(Line::from(vec![
            Span::raw(body_indent.clone()),
            Span::styled("Press ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "[y] ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("approve · ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "[d] ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled("deny", Style::default().fg(Color::DarkGray)),
        ]));
    }

    // Output body — ANSI-parsed so colors survive. Prepend our indent
    // to each line's existing spans so wrap still happens past the margin.
    let body = body_lines(tool);
    let total_body = body.len();
    let shown: Vec<Line<'static>> = body.into_iter().take(MAX_BODY_LINES).collect();
    for line in shown {
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::raw(body_indent.clone()));
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    if total_body > MAX_BODY_LINES {
        out.push(Line::from(vec![
            Span::raw(body_indent.clone()),
            Span::styled(
                format!("… {} more line(s) truncated", total_body - MAX_BODY_LINES),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
    }

    out
}

/// The body rows (after the header) — progress messages, completed
/// output, cancellation messages. All text is run through the ANSI
/// parser so git/cargo/npm colors survive.
fn body_lines(tool: &ToolInfo) -> Vec<Line<'static>> {
    match &tool.state {
        ToolState::Executing {
            progress: Some(p), ..
        } => parse_ansi(p),
        ToolState::Completed {
            output: Some(output),
            ..
        } => parse_ansi(output),
        ToolState::Cancelled {
            message: Some(m), ..
        } => {
            let mut out = parse_ansi(m);
            if out.is_empty() {
                out.push(Line::from(Span::styled(
                    "cancelled",
                    Style::default().fg(Color::Red),
                )));
            }
            out
        }
        _ => Vec::new(),
    }
}

fn header_icon(state: &ToolState, spinner: &str) -> (String, Style) {
    match state {
        ToolState::Streaming { .. } | ToolState::Executing { .. } => (
            spinner.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        ToolState::WaitingConfirmation { .. } => (
            "⚠".to_string(),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        ToolState::Completed { success: true, .. } => (
            "✓".to_string(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        ToolState::Completed { success: false, .. } => (
            "✕".to_string(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        ToolState::Cancelled { .. } => (
            "✕".to_string(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
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

/// Best-effort compact preview of the tool input. Common Anthropic tool
/// shapes have one informative field; pick it, truncate to 120 chars.
fn input_preview(state: &ToolState) -> Option<String> {
    let input = match state {
        ToolState::Streaming {
            partial_input: Some(v),
        }
        | ToolState::WaitingConfirmation { input: v, .. } => v,
        _ => return None,
    };

    let obj = input.as_object()?;

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
    if let Some(Value::String(path)) = obj.get("filepath").or_else(|| obj.get("filePath")) {
        return Some(truncate(path, 120));
    }

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
