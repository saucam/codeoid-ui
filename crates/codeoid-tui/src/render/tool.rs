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

/// Max body lines we show per tool call when `verbose` is on. Large
/// `git log`s or `cargo build` dumps still render their head; the
/// remainder collapses to a "+N more" tail so the transcript doesn't
/// get drowned even in verbose mode.
const MAX_BODY_LINES_VERBOSE: usize = 40;
/// Default ("collapsed") body line cap. Mirrors the web UI so user/
/// assistant turns stay readable when the agent does long-running tool
/// calls. Press `v` in transcript focus to flip to verbose.
const MAX_BODY_LINES_COLLAPSED: usize = 8;

/// Hard ceiling on the bytes fed to the ANSI parser per body render.
/// The line cap below normally keeps parses tiny, but a single
/// `\r`-rewritten progress line (pip/cargo download bars) can accumulate
/// megabytes with zero newlines. Animating blocks bypass the render cache
/// and re-render at 10 Hz — an unbounded parse there is a CPU fire.
/// When the ceiling hits we keep the TAIL: `\r` semantics mean the final
/// rewrite is what a terminal would show anyway.
const MAX_PARSE_BYTES: usize = 256 * 1024;

/// Render a tool invocation. `indent` is the margin applied to the
/// header; body content sits at `indent + "  "` so it reads as a
/// continuation.
///
/// `expanded` reflects either the global verbose override or this
/// individual block being explicitly expanded by the user (via `Enter`
/// while it's the selected block) — either way the body cap goes from
/// the 8-line collapsed preview to the 40-line verbose ceiling.
///
/// `selected` highlights this as the active block in the `[`/`]`
/// navigation cursor — surfaced as a subtle prefix marker on the
/// header so users know which block `Enter` is going to toggle.
pub fn render_tool_block(
    tool: &ToolInfo,
    anim_tick: u64,
    indent: &str,
    expanded: bool,
    selected: bool,
) -> Vec<Line<'static>> {
    let max_body = if expanded {
        MAX_BODY_LINES_VERBOSE
    } else {
        MAX_BODY_LINES_COLLAPSED
    };
    let spinner = SpinnerFrame::for_tick(anim_tick).glyph();
    let body_indent = format!("{indent}  ");

    let mut out: Vec<Line<'static>> = Vec::new();

    // Header: "  ⠋ bash  ·  running · 2.3s"
    // When `selected`, prepend a cyan `▶ ` marker and render the tool
    // name with the accent colour so the user knows this is the block
    // `Enter` will toggle. The marker takes the place of the usual
    // leading space inside `indent` so the column where the tool name
    // appears stays aligned with non-selected rows.
    let (icon, icon_style) = header_icon(&tool.state, spinner);
    let _ = seed_from(&tool.tool_id); // keep usage marker
    let mut header_spans: Vec<Span<'static>> = Vec::with_capacity(8);
    if selected {
        let trimmed = if indent.len() >= 2 {
            indent[..indent.len() - 2].to_owned()
        } else {
            String::new()
        };
        header_spans.push(Span::raw(trimmed));
        header_spans.push(Span::styled(
            "▶ ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        header_spans.push(Span::raw(indent.to_owned()));
    }
    header_spans.push(Span::styled(icon, icon_style));
    header_spans.push(Span::raw(" "));
    header_spans.push(Span::styled(
        tool.name.clone(),
        if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        },
    ));
    header_spans.push(Span::styled("  ·  ", Style::default().fg(Color::DarkGray)));
    header_spans.push(phase_summary(tool, anim_tick));
    out.push(Line::from(header_spans));

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

    // Waiting-for-approval: description + key prompt. For ExitPlanMode
    // we render the plan content (markdown-ish, with the indent
    // prefixed to every line) so the user can actually read what
    // they're about to approve.
    if let ToolState::WaitingConfirmation {
        description, input, ..
    } = &tool.state
    {
        let is_plan_mode = tool.name == "ExitPlanMode" || tool.name == "exit_plan_mode";
        if is_plan_mode {
            if let Some(plan) = input.get("plan").and_then(|p| p.as_str()) {
                out.push(Line::from(vec![
                    Span::raw(body_indent.clone()),
                    Span::styled(
                        "📋 Proposed plan",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                for raw in plan.lines() {
                    out.push(Line::from(vec![
                        Span::raw(body_indent.clone()),
                        Span::styled(raw.to_string(), Style::default().fg(Color::Gray)),
                    ]));
                }
                out.push(Line::raw(""));
            } else {
                let mut desc_spans = vec![
                    Span::raw(body_indent.clone()),
                    Span::styled("⎯ ", Style::default().fg(Color::Magenta)),
                ];
                desc_spans.extend(inline_spans(description, Style::default()));
                out.push(Line::from(desc_spans));
            }
        } else {
            let mut desc_spans = vec![
                Span::raw(body_indent.clone()),
                Span::styled("⎯ ", Style::default().fg(Color::Magenta)),
            ];
            desc_spans.extend(inline_spans(description, Style::default()));
            out.push(Line::from(desc_spans));
        }
        out.push(Line::from(vec![
            Span::raw(body_indent.clone()),
            Span::styled("Press ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "[y] ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if is_plan_mode {
                    "approve plan · "
                } else {
                    "approve · "
                },
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "[d] ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if is_plan_mode { "cancel" } else { "deny" },
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        if is_plan_mode {
            out.push(Line::from(vec![
                Span::raw(body_indent.clone()),
                Span::styled(
                    "  or just type your changes — Claude reads it as the refinement.",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
    }

    // Output body — ANSI-parsed so colors survive. Prepend our indent
    // to each line's existing spans so wrap still happens past the margin.
    // The parse itself is capped at `max_body` lines: only what we show is
    // parsed, the rest is a newline count (`total_body`).
    let (body, total_body) = body_lines(tool, max_body);
    let shown: Vec<Line<'static>> = body.into_iter().take(max_body).collect();
    for line in shown {
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
        spans.push(Span::raw(body_indent.clone()));
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    if total_body > max_body {
        let tail = if expanded {
            format!("… {} more line(s) truncated", total_body - max_body)
        } else if selected {
            format!(
                "… {} more line(s) hidden — press Enter to expand",
                total_body - max_body,
            )
        } else {
            format!(
                "… {} more line(s) hidden — [/] to navigate, Enter to expand",
                total_body - max_body,
            )
        };
        out.push(Line::from(vec![
            Span::raw(body_indent.clone()),
            Span::styled(
                tail,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
    }

    out
}

/// The body rows (after the header) — progress messages, completed
/// output, cancellation messages, run through the ANSI parser so
/// git/cargo/npm colors survive. Returns `(lines, total)` where `lines`
/// holds at most `keep` parsed rows and `total` is the full logical line
/// count — the parse cost is O(shown), not O(accumulated output), which
/// matters because animating blocks re-render at 10 Hz.
fn body_lines(tool: &ToolInfo, keep: usize) -> (Vec<Line<'static>>, usize) {
    let capped = |text: &str| {
        let (slice, total) = cap_lines(text, keep);
        (parse_ansi(slice), total)
    };
    match &tool.state {
        ToolState::Executing {
            progress: Some(p), ..
        } => capped(p),
        ToolState::Completed {
            output: Some(output),
            ..
        } => capped(output),
        ToolState::Cancelled {
            message: Some(m), ..
        } => {
            let (mut out, total) = capped(m);
            if out.is_empty() {
                out.push(Line::from(Span::styled(
                    "cancelled",
                    Style::default().fg(Color::Red),
                )));
                return (out, 1);
            }
            (out, total)
        }
        _ => (Vec::new(), 0),
    }
}

/// Slice `text` to its first `keep` logical lines (plus a byte ceiling for
/// pathological single lines) and count the total lines — one byte scan,
/// zero allocations. Line semantics match [`parse_ansi`]: only `\n`
/// creates a new line (`\r` rewrites the current one).
fn cap_lines(text: &str, keep: usize) -> (&str, usize) {
    let mut newlines = 0usize;
    let mut cut = None;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            newlines += 1;
            if newlines == keep && cut.is_none() {
                cut = Some(i + 1);
            }
        }
    }
    let total = newlines + usize::from(!text.is_empty() && !text.ends_with('\n'));
    let mut slice = cut.map_or(text, |c| &text[..c]);
    if slice.len() > MAX_PARSE_BYTES {
        // A `\r`-progress line megabytes long with no newlines: parse only
        // the tail — the final rewrite is what a terminal would display.
        let mut start = slice.len() - MAX_PARSE_BYTES;
        while !slice.is_char_boundary(start) {
            start += 1;
        }
        slice = &slice[start..];
    }
    (slice, total)
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

#[cfg(test)]
mod tests {
    use super::*;
    use codeoid_protocol::{ToolInfo, ToolState};

    #[test]
    fn cap_lines_keeps_head_and_counts_all() {
        let text = "l1\nl2\nl3\nl4\nl5";
        let (slice, total) = cap_lines(text, 2);
        assert_eq!(slice, "l1\nl2\n");
        assert_eq!(total, 5);
        // Under the cap: whole text untouched.
        let (slice, total) = cap_lines(text, 10);
        assert_eq!(slice, text);
        assert_eq!(total, 5);
        // Trailing newline doesn't add a phantom line; empty is zero.
        assert_eq!(cap_lines("a\nb\n", 10).1, 2);
        assert_eq!(cap_lines("", 10).1, 0);
    }

    #[test]
    fn cap_lines_tails_a_monster_single_line() {
        // A `\r`-rewritten progress line with no newlines: parse only the
        // tail so a 10 Hz animation repaint can't be O(accumulated bytes).
        let text = "x".repeat(MAX_PARSE_BYTES + 50_000);
        let (slice, total) = cap_lines(&text, 8);
        assert_eq!(total, 1);
        assert_eq!(slice.len(), MAX_PARSE_BYTES);
        assert!(text.ends_with(slice));
    }

    #[test]
    fn body_lines_parses_only_what_is_shown_but_reports_exact_total() {
        let progress: String = (0..100).map(|i| format!("row {i}\n")).collect();
        let tool = ToolInfo {
            tool_id: "t1".into(),
            name: "Bash".into(),
            state: ToolState::Executing {
                progress: Some(progress),
                elapsed_ms: Some(10),
            },
        };
        let (lines, total) = body_lines(&tool, 8);
        assert_eq!(lines.len(), 8, "parse capped at what the block shows");
        assert_eq!(total, 100, "the +N-more label still gets the real count");
    }

    #[test]
    fn render_tool_block_truncation_label_matches_capped_parse() {
        let progress: String = (0..30).map(|i| format!("out {i}\n")).collect();
        let tool = ToolInfo {
            tool_id: "t1".into(),
            name: "Bash".into(),
            state: ToolState::Executing {
                progress: Some(progress),
                elapsed_ms: Some(10),
            },
        };
        let lines = render_tool_block(&tool, 0, "", false, false);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.clone().into_owned())
            .collect();
        // Collapsed cap is 8 → 22 hidden.
        assert!(text.contains("22 more line(s) hidden"), "{text}");
        // The shown rows are the HEAD of the output, as before the cap.
        assert!(text.contains("out 0"));
        assert!(!text.contains("out 29"));
    }
}
