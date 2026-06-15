//! Minimal, streaming-safe markdown → Ratatui `Line` renderer.
//!
//! This is intentionally not a full CommonMark implementation — the daemon
//! emits incremental text chunks, and wrestling with a pull parser on
//! partial input is a losing game. We handle the markdown that actually
//! shows up in LLM output:
//!
//! * ATX headers `# ` / `## ` / `### `
//! * Fenced code blocks ```` ``` ```` (with optional language tag)
//! * Inline `code`, **bold**, *italic*
//! * Unordered lists `- ` / `* `
//! * Blockquotes `> `
//! * Horizontal rules `---`
//!
//! Everything else falls through as plain text. Lines are emitted one at a
//! time so partial messages render cleanly during streaming.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render a multi-line markdown string into styled `Line`s.
///
/// `indent` is a prefix string (typically `"  "`) prepended to every line.
#[must_use]
pub fn render_markdown_block(text: &str, indent: &str) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(text.len() / 40 + 1);
    let mut in_fence = false;

    for raw in text.split('\n') {
        // Fenced code blocks.
        if let Some(rest) = raw.strip_prefix("```") {
            if in_fence {
                in_fence = false;
                out.push(Line::from(vec![
                    Span::raw(indent.to_owned()),
                    Span::styled("└─", Style::default().fg(Color::DarkGray)),
                ]));
            } else {
                in_fence = true;
                let lang = rest.trim();
                let lang_label = if lang.is_empty() { "code" } else { lang };
                out.push(Line::from(vec![
                    Span::raw(indent.to_owned()),
                    Span::styled("┌─ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        lang_label.to_string(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
            continue;
        }

        if in_fence {
            out.push(Line::from(vec![
                Span::raw(indent.to_owned()),
                Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    raw.to_string(),
                    Style::default().fg(Color::Rgb(180, 230, 180)),
                ),
            ]));
            continue;
        }

        // Headers.
        if let Some(line) = render_header(raw, indent) {
            out.push(line);
            continue;
        }

        // Horizontal rule.
        if raw.trim() == "---" || raw.trim() == "***" {
            out.push(Line::from(vec![
                Span::raw(indent.to_owned()),
                Span::styled("─".repeat(40), Style::default().fg(Color::DarkGray)),
            ]));
            continue;
        }

        // Blockquote.
        if let Some(rest) = raw.strip_prefix("> ") {
            let mut spans = vec![
                Span::raw(indent.to_owned()),
                Span::styled("▐ ", Style::default().fg(Color::Magenta)),
            ];
            spans.extend(inline_spans(
                rest,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ));
            out.push(Line::from(spans));
            continue;
        }

        // List bullet.
        if let Some(rest) = raw.strip_prefix("- ").or_else(|| raw.strip_prefix("* ")) {
            let mut spans = vec![
                Span::raw(indent.to_owned()),
                Span::styled("• ", Style::default().fg(Color::Cyan)),
            ];
            spans.extend(inline_spans(rest, Style::default()));
            out.push(Line::from(spans));
            continue;
        }

        // Plain paragraph — still apply inline styles.
        let mut spans = vec![Span::raw(indent.to_owned())];
        spans.extend(inline_spans(raw, Style::default()));
        out.push(Line::from(spans));
    }

    out
}

fn render_header(line: &str, indent: &str) -> Option<Line<'static>> {
    let (hashes, rest) = strip_hashes(line)?;
    let style = match hashes {
        1 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    };
    Some(Line::from(vec![
        Span::raw(indent.to_owned()),
        Span::styled(rest.to_string(), style),
    ]))
}

fn strip_hashes(line: &str) -> Option<(usize, &str)> {
    let count = line.chars().take_while(|c| *c == '#').count();
    if count == 0 || count > 6 {
        return None;
    }
    let rest = &line[count..];
    let rest = rest.strip_prefix(' ')?;
    Some((count, rest))
}

/// Render inline `code`, **bold**, *italic* inside a line. Characters that
/// don't participate in any of those patterns fall through verbatim.
pub fn inline_spans(line: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut chars = line.chars().peekable();

    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), base));
        }
    };

    while let Some(c) = chars.next() {
        match c {
            '`' => {
                flush(&mut buf, &mut spans);
                let mut code = String::new();
                while let Some(&n) = chars.peek() {
                    if n == '`' {
                        chars.next();
                        break;
                    }
                    code.push(n);
                    chars.next();
                }
                spans.push(Span::styled(
                    code,
                    Style::default()
                        .fg(Color::Rgb(220, 220, 170))
                        .bg(Color::Rgb(40, 40, 50)),
                ));
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                flush(&mut buf, &mut spans);
                let mut bold = String::new();
                while let Some(&n) = chars.peek() {
                    if n == '*' && peek_second(&chars) == Some('*') {
                        chars.next();
                        chars.next();
                        break;
                    }
                    bold.push(n);
                    chars.next();
                }
                spans.push(Span::styled(bold, base.add_modifier(Modifier::BOLD)));
            }
            '*' | '_' => {
                flush(&mut buf, &mut spans);
                let delim = c;
                let mut italic = String::new();
                while let Some(&n) = chars.peek() {
                    if n == delim {
                        chars.next();
                        break;
                    }
                    italic.push(n);
                    chars.next();
                }
                spans.push(Span::styled(italic, base.add_modifier(Modifier::ITALIC)));
            }
            _ => buf.push(c),
        }
    }
    flush(&mut buf, &mut spans);
    spans
}

fn peek_second(chars: &std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    let mut clone = chars.clone();
    clone.next();
    clone.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_roundtrips() {
        let lines = render_markdown_block("hello world", "");
        assert_eq!(lines.len(), 1);
        let joined: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "hello world");
    }

    #[test]
    fn header_gets_styled() {
        let lines = render_markdown_block("## Heading", "");
        assert_eq!(lines.len(), 1);
        assert!(lines[0]
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "Heading"));
    }

    #[test]
    fn fenced_code_emits_frame() {
        let src = "```rs\nfn main() {}\n```";
        let lines = render_markdown_block(src, "");
        assert_eq!(lines.len(), 3);
        // opener
        assert!(lines[0].spans.iter().any(|s| s.content.contains("rs")));
        // body
        assert!(lines[1]
            .spans
            .iter()
            .any(|s| s.content.as_ref() == "fn main() {}"));
        // closer
        assert!(lines[2].spans.iter().any(|s| s.content.as_ref() == "└─"));
    }

    #[test]
    fn inline_code_and_bold() {
        let spans = inline_spans("call **foo** via `bar`", Style::default());
        // "call ", "foo" bold, " via ", "bar" code
        let texts: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(texts.contains(&"foo"));
        assert!(texts.contains(&"bar"));
    }
}
