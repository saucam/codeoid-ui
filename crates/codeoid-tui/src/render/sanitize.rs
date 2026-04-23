//! Scrub tool output and agent-supplied text before handing it to ratatui.
//!
//! Raw tool output routinely contains:
//!
//! * **ANSI escape sequences** — `git status` / `ls --color` / build
//!   tools emit SGR codes (`\x1b[31m…\x1b[0m`) that ratatui cannot
//!   interpret. Left in place, they either pass through to the terminal
//!   (which happens to render them correctly but causes ratatui's layout
//!   width to diverge from visual width, producing cell-misalignment
//!   elsewhere) or get rendered as literal `ESC[31m` noise. Both are bad.
//!
//! * **Carriage returns** — progress bars (`pip install`, `git clone`)
//!   stream `\r` to rewrite the current line. In a ratatui row, a `\r`
//!   in a `Span`'s content would reach the terminal and move the cursor
//!   to column 0, so subsequent characters overwrite earlier ones on the
//!   same visual row. This is the root cause of the "garbled text"
//!   screenshots.
//!
//! * **NUL, bell, and other C0 controls** — rare in practice but
//!   dangerous: NUL truncates the line in some terminals, BEL triggers
//!   an audible alert, and a lone `ESC` can leave the terminal waiting
//!   for a CSI sequence that never arrives.
//!
//! The sanitizer keeps `\n` (newline) and `\t` (tab) — both have
//! well-defined layout behaviour in ratatui's `Text` handling — and
//! drops everything else.

/// Strip ANSI escape sequences and problematic control characters.
///
/// Preserves:
/// * `\n` — callers typically split on it first, but the sanitizer
///   doesn't assume that
/// * `\t` — rendered as a ratatui-aware tab stop
/// * All printable Unicode
///
/// Strips:
/// * CSI sequences: `ESC [ … final-letter` (SGR colors, cursor moves)
/// * OSC sequences: `ESC ] … BEL` or `ESC ] … ESC \`
/// * Single-byte C0 controls except `\n` and `\t`
/// * Carriage returns
///
/// This is a streaming parser, not a full terminal emulator. It will
/// strip malformed sequences by consuming until a plausible terminator,
/// which is fine — we'd rather drop a few display-affecting bytes than
/// risk leaking them to the real terminal.
#[must_use]
pub fn sanitize_for_display(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // ESC begins a control sequence. Consume until the sequence
            // terminator, then drop the whole thing.
            '\x1b' => consume_escape_sequence(&mut chars),

            // Drop bare carriage returns — the single biggest offender
            // for display corruption.
            '\r' => {}

            // Preserve whitespace that ratatui handles correctly.
            '\n' | '\t' => out.push(c),

            // Drop every other C0 control. Unicode properties correctly
            // classify BEL, VT, FF, BS, etc. here.
            c if c.is_control() => {}

            // Keep everything printable.
            c => out.push(c),
        }
    }
    out
}

/// Consume characters from `chars` up to and including the terminator of
/// an escape sequence. Handles the two common shapes:
///
/// * **CSI** (`ESC [ … letter`) — used by SGR color codes and cursor
///   movement. Terminator is an ASCII letter (`@` through `~`).
/// * **OSC** (`ESC ] … BEL` or `ESC ] … ESC \`) — used for window
///   titles and hyperlinks. Terminators are BEL or ST (ESC backslash).
/// * **Other** (`ESC letter`, e.g. `ESC 7` / `ESC 8` for cursor
///   save/restore) — consume the single next char.
fn consume_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let Some(&next) = chars.peek() else { return };

    match next {
        '[' => {
            // CSI — consume params until a final byte in 0x40-0x7E.
            chars.next();
            while let Some(&c) = chars.peek() {
                chars.next();
                let code = c as u32;
                if (0x40..=0x7E).contains(&code) {
                    return;
                }
            }
        }
        ']' => {
            // OSC — consume until BEL or ST.
            chars.next();
            while let Some(&c) = chars.peek() {
                chars.next();
                if c == '\x07' {
                    return;
                }
                if c == '\x1b' {
                    if let Some(&'\\') = chars.peek() {
                        chars.next();
                    }
                    return;
                }
            }
        }
        _ => {
            // Two-byte escape — consume the second byte.
            chars.next();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_passes_through() {
        assert_eq!(sanitize_for_display("hello world"), "hello world");
    }

    #[test]
    fn newlines_preserved() {
        assert_eq!(sanitize_for_display("a\nb\nc"), "a\nb\nc");
    }

    #[test]
    fn tabs_preserved() {
        assert_eq!(sanitize_for_display("col1\tcol2"), "col1\tcol2");
    }

    #[test]
    fn carriage_return_stripped() {
        // The specific bug the user reported: git progress output with
        // `\r` caused visible text overlap because ratatui passed the CR
        // to the terminal.
        assert_eq!(sanitize_for_display("first\rsecond"), "firstsecond");
    }

    #[test]
    fn crlf_becomes_lf() {
        assert_eq!(sanitize_for_display("line1\r\nline2"), "line1\nline2");
    }

    #[test]
    fn sgr_color_codes_stripped() {
        let colored = "\x1b[31mred\x1b[0m normal";
        assert_eq!(sanitize_for_display(colored), "red normal");
    }

    #[test]
    fn multi_param_sgr_stripped() {
        let colored = "\x1b[1;31;40mbold red on black\x1b[0m";
        assert_eq!(sanitize_for_display(colored), "bold red on black");
    }

    #[test]
    fn cursor_movement_stripped() {
        // `ESC[2K` = clear line, `ESC[G` = go to column 0
        let s = "before\x1b[2K\x1b[Gafter";
        assert_eq!(sanitize_for_display(s), "beforeafter");
    }

    #[test]
    fn osc_hyperlink_stripped() {
        // `ESC]8;;https://example.com\x07text\x1b]8;;\x07`
        let linked = "\x1b]8;;https://example.com\x07click\x1b]8;;\x07";
        assert_eq!(sanitize_for_display(linked), "click");
    }

    #[test]
    fn bell_and_other_c0_stripped() {
        let noisy = "loud\x07quiet\x00zero\x08back";
        assert_eq!(sanitize_for_display(noisy), "loudquietzeroback");
    }

    #[test]
    fn malformed_csi_consumes_reasonably() {
        // No terminator at end of string — should consume to EOF and
        // produce empty output for that fragment, rather than leaking ESC.
        let s = "safe\x1b[99";
        assert_eq!(sanitize_for_display(s), "safe");
    }

    #[test]
    fn lone_escape_consumes_next_char() {
        // `ESC 7` = save cursor. Two bytes, no CSI bracket.
        let s = "a\x1b7b";
        assert_eq!(sanitize_for_display(s), "ab");
    }

    #[test]
    fn unicode_preserved() {
        assert_eq!(sanitize_for_display("héllo → 世界"), "héllo → 世界");
    }
}
