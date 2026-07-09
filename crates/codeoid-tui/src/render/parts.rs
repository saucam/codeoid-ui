//! Rich `parts[]` renderer — the protocol's `ContentPart` union as styled
//! lines. Providers emit these on standalone messages (`custom_message`
//! events: status cards, diffs, tables, buttons). Unknown kinds render
//! nothing — the wire contract is "ignore unknown kinds", and the parent
//! message's `content` fallback still shows.
//!
//! Buttons render as visible chips with their action name. Activating them
//! (`session.part_action`) needs a focus/selection model this renderer
//! doesn't own yet — the web UI is the interactive surface for now.

use codeoid_protocol::message::TreeNode;
use codeoid_protocol::ContentPart;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::markdown::render_markdown_block;
use super::sanitize::sanitize_for_display;

/// True when a message's parts carry something the plain `content` string
/// doesn't already cover. The daemon mirrors simple text into a single text
/// part on commit — rendering THAT via the parts path would bypass the
/// markdown pipeline for no gain, so callers keep the legacy path for it.
#[must_use]
pub fn has_rich_parts(parts: Option<&Vec<ContentPart>>) -> bool {
    parts
        .is_some_and(|p| p.len() > 1 || !matches!(p.first(), None | Some(ContentPart::Text { .. })))
}

/// Render every known part to indented lines, in order.
#[must_use]
pub fn render_parts(parts: &[ContentPart], indent: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for part in parts {
        render_part(part, indent, &mut out);
    }
    out
}

fn render_part(part: &ContentPart, indent: &str, out: &mut Vec<Line<'static>>) {
    match part {
        ContentPart::Text { text, markdown } => {
            let clean = sanitize_for_display(text);
            if markdown.unwrap_or(true) {
                out.extend(render_markdown_block(&clean, indent));
            } else {
                for raw in clean.lines() {
                    out.push(Line::from(vec![
                        Span::raw(indent.to_string()),
                        Span::styled(raw.to_string(), Style::default().fg(Color::White)),
                    ]));
                }
            }
        }
        ContentPart::Code {
            code, file_path, ..
        } => {
            if let Some(path) = file_path {
                out.push(Line::from(vec![
                    Span::raw(indent.to_string()),
                    Span::styled(path.clone(), Style::default().fg(Color::DarkGray)),
                ]));
            }
            for raw in sanitize_for_display(code).lines() {
                out.push(Line::from(vec![
                    Span::raw(indent.to_string()),
                    Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(raw.to_string(), Style::default().fg(Color::Gray)),
                ]));
            }
        }
        ContentPart::FileRef {
            path,
            lines,
            change,
        } => {
            let mut spans = vec![
                Span::raw(indent.to_string()),
                Span::styled("📄 ", Style::default()),
                Span::styled(path.clone(), Style::default().fg(Color::Cyan)),
            ];
            if let Some([start, end]) = lines {
                spans.push(Span::styled(
                    format!(":{start}–{end}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if let Some(change) = change {
                spans.push(Span::styled(
                    format!("  +{}", change.added),
                    Style::default().fg(Color::Green),
                ));
                spans.push(Span::styled(
                    format!(" −{}", change.removed),
                    Style::default().fg(Color::Red),
                ));
            }
            out.push(Line::from(spans));
        }
        ContentPart::Diff {
            path,
            added,
            removed,
            ..
        } => {
            out.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(path.clone(), Style::default().fg(Color::White)),
                Span::styled(format!("  +{added}"), Style::default().fg(Color::Green)),
                Span::styled(format!(" −{removed}"), Style::default().fg(Color::Red)),
            ]));
        }
        ContentPart::Tree { label, children } => {
            out.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(
                    label.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            render_tree_nodes(children, indent, 1, out);
        }
        ContentPart::Button {
            label,
            action,
            style,
            ..
        } => {
            let color = match style {
                Some(codeoid_protocol::message::ButtonStyle::Danger) => Color::Red,
                Some(codeoid_protocol::message::ButtonStyle::Primary) => Color::Cyan,
                _ => Color::Gray,
            };
            out.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(
                    format!("[ {label} ]"),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ({action} — activate from the web UI)"),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
        ContentPart::Progress {
            message, percent, ..
        } => {
            let mut spans = vec![Span::raw(indent.to_string())];
            if let Some(pct) = percent {
                let filled = usize::from(*pct.min(&100)) / 10;
                let bar: String = "█".repeat(filled) + &"░".repeat(10 - filled);
                spans.push(Span::styled(bar, Style::default().fg(Color::Cyan)));
                spans.push(Span::styled(
                    format!(" {pct:>3}% "),
                    Style::default().fg(Color::DarkGray),
                ));
            } else {
                spans.push(Span::styled("⏳ ", Style::default()));
            }
            spans.push(Span::styled(
                message.clone(),
                Style::default().fg(Color::Gray),
            ));
            out.push(Line::from(spans));
        }
        ContentPart::Image { url, alt } => {
            out.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled("🖼 ", Style::default()),
                Span::styled(
                    alt.clone().unwrap_or_else(|| "image".to_string()),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(format!("  {url}"), Style::default().fg(Color::DarkGray)),
            ]));
        }
        ContentPart::Anchor { uri, title } => {
            out.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(
                    title.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Span::styled(format!("  {uri}"), Style::default().fg(Color::DarkGray)),
            ]));
        }
        ContentPart::Table { headers, rows } => {
            render_table(headers, rows, indent, out);
        }
        ContentPart::Unknown => {
            // Additive wire evolution: newer part kinds render nothing here;
            // the parent message's `content` fallback carries the info.
        }
    }
}

fn render_tree_nodes(nodes: &[TreeNode], indent: &str, depth: usize, out: &mut Vec<Line<'static>>) {
    for node in nodes {
        let glyph = match node.kind {
            codeoid_protocol::message::TreeNodeType::Directory => "📁",
            codeoid_protocol::message::TreeNodeType::File => "· ",
        };
        out.push(Line::from(vec![
            Span::raw(format!("{indent}{}", "  ".repeat(depth))),
            Span::styled(
                format!("{glyph} {}", node.label),
                Style::default().fg(Color::Gray),
            ),
        ]));
        if let Some(children) = &node.children {
            render_tree_nodes(children, indent, depth + 1, out);
        }
    }
}

/// Simple column-aligned table. Column widths fit the widest cell; the
/// terminal's own wrapping handles overflow on narrow screens.
fn render_table(
    headers: &[String],
    rows: &[Vec<String>],
    indent: &str,
    out: &mut Vec<Line<'static>>,
) {
    let cols = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if cols == 0 {
        return;
    }
    let mut widths = vec![0usize; cols];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = widths[i].max(h.chars().count());
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < cols {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }

    let fmt_row = |cells: &[String], style: Style| -> Line<'static> {
        let mut spans = vec![Span::raw(indent.to_string())];
        for (i, width) in widths.iter().enumerate() {
            let cell = cells.get(i).map_or("", String::as_str);
            spans.push(Span::styled(format!("{cell:<width$}"), style));
            if i + 1 < cols {
                spans.push(Span::styled("  │ ", Style::default().fg(Color::DarkGray)));
            }
        }
        Line::from(spans)
    };

    out.push(fmt_row(
        headers,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ));
    let rule_width: usize = widths.iter().sum::<usize>() + (cols - 1) * 4;
    out.push(Line::from(vec![
        Span::raw(indent.to_string()),
        Span::styled("─".repeat(rule_width), Style::default().fg(Color::DarkGray)),
    ]));
    for row in rows {
        out.push(fmt_row(row, Style::default().fg(Color::Gray)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> ContentPart {
        ContentPart::Text {
            text: t.to_string(),
            markdown: Some(false),
        }
    }

    #[test]
    fn rich_detection_ignores_single_text_part() {
        assert!(!has_rich_parts(None));
        assert!(!has_rich_parts(Some(&vec![])));
        assert!(!has_rich_parts(Some(&vec![text("hi")])));
        assert!(has_rich_parts(Some(&vec![
            text("hi"),
            ContentPart::Diff {
                path: "a.rs".into(),
                added: 1,
                removed: 2,
                original_path: None,
            }
        ])));
        assert!(has_rich_parts(Some(&vec![ContentPart::Table {
            headers: vec!["a".into()],
            rows: vec![vec!["1".into()]],
        }])));
    }

    #[test]
    fn renders_each_known_kind_to_lines() {
        let parts = vec![
            text("plain"),
            ContentPart::Code {
                code: "let x = 1;".into(),
                language: Some("rust".into()),
                file_path: Some("src/x.rs".into()),
            },
            ContentPart::Diff {
                path: "y.rs".into(),
                added: 3,
                removed: 1,
                original_path: None,
            },
            ContentPart::Table {
                headers: vec!["name".into(), "value".into()],
                rows: vec![vec!["pi".into(), "3".into()]],
            },
            ContentPart::Progress {
                message: "indexing".into(),
                percent: Some(40),
                elapsed_ms: None,
            },
            ContentPart::Button {
                label: "Deploy".into(),
                action: "deploy".into(),
                data: None,
                style: None,
            },
            ContentPart::Anchor {
                uri: "https://example.com".into(),
                title: "docs".into(),
            },
        ];
        let lines = render_parts(&parts, "  ");
        let flat: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.clone()))
            .collect();
        assert!(flat.contains("plain"));
        assert!(flat.contains("let x = 1;"));
        assert!(flat.contains("src/x.rs"));
        assert!(flat.contains("+3"));
        assert!(flat.contains("name"));
        assert!(flat.contains("indexing"));
        assert!(flat.contains("[ Deploy ]"));
        assert!(flat.contains("https://example.com"));
    }

    #[test]
    fn table_columns_align_to_widest_cell() {
        let mut out = Vec::new();
        render_table(
            &["id".into(), "name".into()],
            &[
                vec!["1".into(), "short".into()],
                vec!["2".into(), "a-much-longer-name".into()],
            ],
            "",
            &mut out,
        );
        // header + rule + 2 rows
        assert_eq!(out.len(), 4);
    }
}
