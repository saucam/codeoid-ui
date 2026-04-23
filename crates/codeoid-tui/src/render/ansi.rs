//! ANSI SGR → ratatui `Line` parser.
//!
//! Turns raw tool output (with `git status` colors, `cargo check` error
//! reds, progress bars that use `\r`, etc.) into styled ratatui lines
//! while dropping non-visual escape sequences (cursor moves, OSC
//! hyperlinks, bell).
//!
//! # Design
//!
//! This is NOT a full terminal emulator — we're rendering settled tool
//! output, not hosting a live PTY. The parser covers the subset that
//! matters for displayed output:
//!
//! * **SGR** (`ESC[...m`) — colors (16 + 256 + truecolor), bold, dim,
//!   italic, underline, reverse, strikethrough, and all their reset codes.
//! * **`\r`** — progress-bar semantics. We discard the in-progress line
//!   and start fresh, so `Downloading 10%\r...\rDownloading 100%` ends up
//!   as `Downloading 100%` rather than all intermediate frames concatenated.
//! * **`\n`** — flush current line, start a new one.
//! * **CSI non-SGR** (`ESC[...letter` where letter ≠ `m`) — consumed and dropped.
//! * **OSC** (`ESC]...BEL` or `ESC]...ESC\`) — consumed and dropped.
//! * **C0 controls** other than `\n` and `\t` — dropped.
//!
//! Malformed sequences (unterminated CSI at EOF, unexpected chars) are
//! consumed-to-sentinel so they don't leak raw ESC bytes into the
//! terminal.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Parse a string containing ANSI SGR codes into styled lines.
///
/// The input is split on `\n`. Within a line, SGR codes change the
/// current style; characters inherit whatever style is active at the
/// time they're emitted. Bare `\r` discards the in-progress line.
#[must_use]
pub fn parse_ansi(input: &str) -> Vec<Line<'static>> {
    let mut parser = AnsiParser::new();
    parser.feed(input);
    parser.finalize()
}

/// Streaming ANSI parser. Public so callers who feed delta chunks (e.g.
/// an assistant message arriving as `content_append` deltas) can keep
/// the style state across feeds.
pub struct AnsiParser {
    style: Style,
    buf: String,
    line_spans: Vec<Span<'static>>,
    lines: Vec<Line<'static>>,
}

impl Default for AnsiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AnsiParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            style: Style::default(),
            buf: String::new(),
            line_spans: Vec::new(),
            lines: Vec::new(),
        }
    }

    pub fn feed(&mut self, input: &str) {
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => self.handle_escape(&mut chars),
                '\r' => self.reset_current_line(),
                '\n' => self.flush_line(),
                '\t' => self.buf.push('\t'),
                c if c.is_control() => {
                    // BEL, NUL, BS — drop. These either make noise or
                    // move the cursor, neither of which we want.
                }
                c => self.buf.push(c),
            }
        }
    }

    #[must_use]
    pub fn finalize(mut self) -> Vec<Line<'static>> {
        if !self.buf.is_empty() || !self.line_spans.is_empty() {
            self.flush_line();
        }
        self.lines
    }

    fn flush_buf(&mut self) {
        if !self.buf.is_empty() {
            let text = std::mem::take(&mut self.buf);
            self.line_spans.push(Span::styled(text, self.style));
        }
    }

    fn flush_line(&mut self) {
        self.flush_buf();
        let spans = std::mem::take(&mut self.line_spans);
        self.lines.push(Line::from(spans));
    }

    fn reset_current_line(&mut self) {
        // \r = "move cursor to column 0". In a real terminal subsequent
        // output overwrites the line in place; approximate by tossing
        // whatever we've built for the current line so far.
        self.buf.clear();
        self.line_spans.clear();
    }

    fn handle_escape(&mut self, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        let Some(&next) = chars.peek() else { return };
        match next {
            '[' => {
                chars.next();
                self.handle_csi(chars);
            }
            ']' => {
                chars.next();
                consume_osc(chars);
            }
            _ => {
                // 2-byte escape (ESC 7, ESC 8, etc.) — consume one and drop.
                chars.next();
            }
        }
    }

    fn handle_csi(&mut self, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        let mut params = String::new();
        while let Some(&c) = chars.peek() {
            chars.next();
            let code = c as u32;
            // Final byte for CSI is anything in 0x40-0x7E (ASCII @ through ~).
            if (0x40..=0x7E).contains(&code) {
                if c == 'm' {
                    // SGR — apply to current style. Anything else (cursor
                    // moves, clear, scroll, etc.) is dropped silently.
                    self.flush_buf();
                    self.apply_sgr(&params);
                }
                return;
            }
            params.push(c);
        }
        // Unterminated at EOF — treat as if never started; buf already
        // flushed no content since no SGR was applied.
    }

    fn apply_sgr(&mut self, params: &str) {
        if params.is_empty() {
            self.style = Style::default();
            return;
        }
        let nums: Vec<i32> = params
            .split(';')
            .map(|p| p.parse::<i32>().unwrap_or(0))
            .collect();

        let mut i = 0;
        while i < nums.len() {
            match nums[i] {
                0 => self.style = Style::default(),
                1 => self.style = self.style.add_modifier(Modifier::BOLD),
                2 => self.style = self.style.add_modifier(Modifier::DIM),
                3 => self.style = self.style.add_modifier(Modifier::ITALIC),
                4 => self.style = self.style.add_modifier(Modifier::UNDERLINED),
                // 5 = slow blink, 6 = rapid blink. Ratatui has one
                // SLOW_BLINK modifier; merge both.
                5 | 6 => self.style = self.style.add_modifier(Modifier::SLOW_BLINK),
                7 => self.style = self.style.add_modifier(Modifier::REVERSED),
                8 => self.style = self.style.add_modifier(Modifier::HIDDEN),
                9 => self.style = self.style.add_modifier(Modifier::CROSSED_OUT),
                22 => {
                    self.style = self
                        .style
                        .remove_modifier(Modifier::BOLD | Modifier::DIM);
                }
                23 => self.style = self.style.remove_modifier(Modifier::ITALIC),
                24 => self.style = self.style.remove_modifier(Modifier::UNDERLINED),
                25 => self.style = self.style.remove_modifier(Modifier::SLOW_BLINK),
                27 => self.style = self.style.remove_modifier(Modifier::REVERSED),
                28 => self.style = self.style.remove_modifier(Modifier::HIDDEN),
                29 => self.style = self.style.remove_modifier(Modifier::CROSSED_OUT),
                n @ 30..=37 => self.style = self.style.fg(std_color(n - 30)),
                38 => {
                    if let Some(c) = parse_ext_color(&nums, &mut i) {
                        self.style = self.style.fg(c);
                    }
                }
                39 => self.style = self.style.fg(Color::Reset),
                n @ 40..=47 => self.style = self.style.bg(std_color(n - 40)),
                48 => {
                    if let Some(c) = parse_ext_color(&nums, &mut i) {
                        self.style = self.style.bg(c);
                    }
                }
                49 => self.style = self.style.bg(Color::Reset),
                n @ 90..=97 => self.style = self.style.fg(bright_color(n - 90)),
                n @ 100..=107 => self.style = self.style.bg(bright_color(n - 100)),
                _ => {} // unknown param — ignore and move on
            }
            i += 1;
        }
    }
}

fn consume_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    // OSC terminates on BEL (\x07) or ST (ESC \).
    while let Some(c) = chars.next() {
        if c == '\x07' {
            return;
        }
        if c == '\x1b' {
            if matches!(chars.peek(), Some(&'\\')) {
                chars.next();
            }
            return;
        }
    }
}

fn std_color(idx: i32) -> Color {
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        _ => Color::Reset,
    }
}

fn bright_color(idx: i32) -> Color {
    match idx {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        7 => Color::White,
        _ => Color::Reset,
    }
}

/// 38;5;N = 256-color ·  38;2;R;G;B = truecolor.  Advances `i` past the
/// consumed params so the outer loop keeps its position correct.
fn parse_ext_color(nums: &[i32], i: &mut usize) -> Option<Color> {
    let mode = *nums.get(*i + 1)?;
    if mode == 5 {
        let n = *nums.get(*i + 2)?;
        if !(0..=255).contains(&n) {
            return None;
        }
        *i += 2;
        Some(Color::Indexed(n as u8))
    } else if mode == 2 {
        let r = *nums.get(*i + 2)?;
        let g = *nums.get(*i + 3)?;
        let b = *nums.get(*i + 4)?;
        if ![r, g, b].iter().all(|v| (0..=255).contains(v)) {
            return None;
        }
        *i += 4;
        Some(Color::Rgb(r as u8, g as u8, b as u8))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_line_text(lines: &[Line<'static>]) -> String {
        lines
            .first()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .unwrap_or_default()
    }

    fn first_line_style(lines: &[Line<'static>], span_idx: usize) -> Style {
        lines
            .first()
            .and_then(|l| l.spans.get(span_idx))
            .map(|s| s.style)
            .unwrap_or_default()
    }

    #[test]
    fn plain_text_one_line() {
        let lines = parse_ansi("hello world");
        assert_eq!(lines.len(), 1);
        assert_eq!(first_line_text(&lines), "hello world");
    }

    #[test]
    fn newline_splits_lines() {
        let lines = parse_ansi("one\ntwo\nthree");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn red_color_applied() {
        let lines = parse_ansi("\x1b[31mred\x1b[0m");
        assert_eq!(lines.len(), 1);
        assert_eq!(first_line_style(&lines, 0).fg, Some(Color::Red));
    }

    #[test]
    fn reset_clears_color() {
        let lines = parse_ansi("\x1b[31mred\x1b[0mnormal");
        // Two spans: "red" in red, "normal" in default.
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].style.fg, Some(Color::Red));
        assert_eq!(spans[1].style.fg, None);
    }

    #[test]
    fn bold_and_color_combined() {
        let lines = parse_ansi("\x1b[1;31mbold red\x1b[0m");
        let style = first_line_style(&lines, 0);
        assert_eq!(style.fg, Some(Color::Red));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn italic_and_underline() {
        let lines = parse_ansi("\x1b[3;4mfancy\x1b[0m");
        let style = first_line_style(&lines, 0);
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn truecolor_24bit() {
        let lines = parse_ansi("\x1b[38;2;255;128;64morange\x1b[0m");
        let style = first_line_style(&lines, 0);
        assert_eq!(style.fg, Some(Color::Rgb(255, 128, 64)));
    }

    #[test]
    fn color_256() {
        let lines = parse_ansi("\x1b[38;5;99mindexed\x1b[0m");
        let style = first_line_style(&lines, 0);
        assert_eq!(style.fg, Some(Color::Indexed(99)));
    }

    #[test]
    fn background_color() {
        let lines = parse_ansi("\x1b[44mbg-blue\x1b[0m");
        let style = first_line_style(&lines, 0);
        assert_eq!(style.bg, Some(Color::Blue));
    }

    #[test]
    fn bright_foreground() {
        let lines = parse_ansi("\x1b[91mbright red\x1b[0m");
        let style = first_line_style(&lines, 0);
        assert_eq!(style.fg, Some(Color::LightRed));
    }

    #[test]
    fn cr_resets_current_line() {
        // Progress-bar semantics: the final state wins.
        let lines = parse_ansi("downloading 10%\rdownloading 20%\rdownloading 100%");
        assert_eq!(lines.len(), 1);
        assert_eq!(first_line_text(&lines), "downloading 100%");
    }

    #[test]
    fn cr_newline_sequences() {
        // \r\n should produce a single line break.
        let lines = parse_ansi("a\r\nb");
        assert_eq!(lines.len(), 2);
        assert_eq!(first_line_text(&lines), "");
        assert_eq!(
            lines[1]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
            "b"
        );
    }

    #[test]
    fn cursor_movement_dropped() {
        // `\x1b[2K` (clear line) and `\x1b[G` (column 1) must not appear
        // in the output.
        let lines = parse_ansi("before\x1b[2K\x1b[Gafter");
        assert_eq!(first_line_text(&lines), "beforeafter");
    }

    #[test]
    fn osc_hyperlink_dropped() {
        let lines = parse_ansi("\x1b]8;;https://x.com\x07click\x1b]8;;\x07");
        assert_eq!(first_line_text(&lines), "click");
    }

    #[test]
    fn bell_and_nul_dropped() {
        let lines = parse_ansi("loud\x07quiet\x00zero");
        assert_eq!(first_line_text(&lines), "loudquietzero");
    }

    #[test]
    fn malformed_csi_at_eof_does_not_leak_esc() {
        let lines = parse_ansi("safe\x1b[99");
        let combined = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>();
        assert!(!combined.contains('\x1b'));
        assert_eq!(combined, "safe");
    }

    #[test]
    fn dim_then_22_resets_dim() {
        // ESC[2m dim on, ESC[22m dim/bold off.
        let lines = parse_ansi("\x1b[2mdim\x1b[22mbold-off");
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 2);
        assert!(spans[0].style.add_modifier.contains(Modifier::DIM));
        assert!(!spans[1].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn colors_persist_across_multiple_lines() {
        // A color turned on before a newline should stay on afterwards
        // until explicitly reset.
        let lines = parse_ansi("\x1b[32mgreen1\ngreen2\x1b[0m");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn tab_preserved() {
        let lines = parse_ansi("col1\tcol2");
        assert_eq!(first_line_text(&lines), "col1\tcol2");
    }

    #[test]
    fn git_status_like_output() {
        // Realistic fixture: `git status` with color.status=always.
        let raw = "On branch \x1b[32mmain\x1b[m\n\
                   Changes not staged for commit:\n\
                   \t\x1b[31mmodified:   Cargo.toml\x1b[m";
        let lines = parse_ansi(raw);
        assert_eq!(lines.len(), 3);
        // Third line: "modified:   Cargo.toml" in red.
        let third = &lines[2];
        let red_span = third
            .spans
            .iter()
            .find(|s| s.content.contains("modified"))
            .expect("modified span");
        assert_eq!(red_span.style.fg, Some(Color::Red));
    }

    #[test]
    fn streaming_parser_preserves_state_across_feeds() {
        let mut parser = AnsiParser::new();
        parser.feed("\x1b[31mpart1");
        parser.feed("part2\x1b[0m");
        let lines = parser.finalize();
        assert_eq!(lines.len(), 1);
        // Both chunks red because ESC[0m arrived only at the end.
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Red));
    }
}
