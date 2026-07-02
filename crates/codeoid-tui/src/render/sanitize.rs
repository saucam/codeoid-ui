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
//! The sanitizer keeps `\n` (newline), expands `\t` to spaces (ratatui
//! 0.29 silently drops tabs as zero-width graphemes, destroying the
//! alignment of `git status` / Makefile output), and drops everything
//! else.

use unicode_width::UnicodeWidthChar;

/// Tab stop width for `\t` expansion. Matches the de-facto terminal
/// default (and `crate::render::wrap::TAB_WIDTH`).
const TAB_WIDTH: usize = 8;

/// Strip ANSI escape sequences and problematic control characters.
///
/// Preserves:
/// * `\n` — callers typically split on it first, but the sanitizer
///   doesn't assume that
/// * All printable Unicode
///
/// Rewrites:
/// * `\t` — expanded to spaces up to the next 8-column tab stop,
///   column-aware within the current line (ratatui 0.29 renders `\t`
///   as zero-width, so leaving it in destroys column alignment)
///
/// Strips:
/// * CSI sequences: `ESC [ … final-letter` (SGR colors, cursor moves),
///   including the 8-bit C1 form (`U+009B … final-letter`)
/// * OSC sequences: `ESC ] … BEL` or `ESC ] … ESC \`
/// * DCS / SOS / PM / APC sequences (`ESC P` / `ESC X` / `ESC ^` /
///   `ESC _` … `ESC \` or BEL) — sixel + tmux-passthrough payloads
///   must not flood the transcript as text
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
    // Visual column within the current output line — drives tab stops.
    let mut col: usize = 0;

    while let Some(c) = chars.next() {
        match c {
            // ESC begins a control sequence. Consume until the sequence
            // terminator, then drop the whole thing.
            '\x1b' => consume_escape_sequence(&mut chars),

            // 8-bit C1 CSI — same grammar as `ESC [`, single-char intro.
            '\u{9b}' => consume_csi_body(&mut chars),

            // Drop bare carriage returns — the single biggest offender
            // for display corruption.
            '\r' => {}

            '\n' => {
                out.push(c);
                col = 0;
            }

            // Expand to the next 8-column stop. Ratatui 0.29 drops `\t`
            // as a zero-width grapheme, so a literal tab would collapse
            // columns instead of aligning them.
            '\t' => {
                let pad = TAB_WIDTH - (col % TAB_WIDTH);
                out.extend(std::iter::repeat_n(' ', pad));
                col += pad;
            }

            // Drop every other C0 control. Unicode properties correctly
            // classify BEL, VT, FF, BS, etc. here.
            c if c.is_control() => {}

            // Keep everything printable.
            c => {
                out.push(c);
                col += UnicodeWidthChar::width(c).unwrap_or(0);
            }
        }
    }
    out
}

/// Consume characters from `chars` up to and including the terminator of
/// an escape sequence. Handles the common shapes:
///
/// * **CSI** (`ESC [ … letter`) — used by SGR color codes and cursor
///   movement. Terminator is an ASCII letter (`@` through `~`).
/// * **OSC** (`ESC ] … BEL` or `ESC ] … ESC \`) — used for window
///   titles and hyperlinks. Terminators are BEL or ST (ESC backslash).
/// * **DCS / SOS / PM / APC** (`ESC P` / `ESC X` / `ESC ^` / `ESC _`)
///   — string sequences carrying an arbitrary payload (sixel images,
///   tmux passthrough). Terminated like OSC; treating these as
///   two-byte escapes would leak the entire payload into the
///   transcript as text.
/// * **Other** (`ESC letter`, e.g. `ESC 7` / `ESC 8` for cursor
///   save/restore) — consume the single next char.
fn consume_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let Some(&next) = chars.peek() else { return };

    match next {
        '[' => {
            chars.next();
            consume_csi_body(chars);
        }
        ']' | 'P' | 'X' | '^' | '_' => {
            chars.next();
            consume_string_sequence(chars);
        }
        _ => {
            // Two-byte escape — consume the second byte.
            chars.next();
        }
    }
}

/// Consume CSI parameter/intermediate bytes up to and including the
/// final byte (0x40–0x7E). Shared by the 7-bit (`ESC [`) and 8-bit
/// (`U+009B`) forms.
fn consume_csi_body(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(&c) = chars.peek() {
        chars.next();
        let code = c as u32;
        if (0x40..=0x7E).contains(&code) {
            return;
        }
    }
}

/// Consume an OSC/DCS/SOS/PM/APC payload up to and including its
/// terminator: ST (`ESC \`) or BEL. (Strictly only OSC accepts BEL, but
/// real-world emitters use it for the others too; consuming a little
/// extra beats flooding the transcript.)
fn consume_string_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
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
    fn tab_expands_to_next_eight_col_stop() {
        // "col1" ends at column 4 → tab pads 4 spaces to column 8.
        assert_eq!(sanitize_for_display("col1\tcol2"), "col1    col2");
    }

    #[test]
    fn tab_at_line_start_is_full_stop() {
        assert_eq!(sanitize_for_display("\tx"), "        x");
    }

    #[test]
    fn tab_at_exact_stop_advances_a_full_stop() {
        // Column 8 is already a stop → tab advances to 16, not 8.
        assert_eq!(sanitize_for_display("12345678\tx"), "12345678        x");
    }

    #[test]
    fn multiple_mid_line_tabs_align_to_stops() {
        // "ab" (2) → pad 6 → col 8; "c" → col 9 → pad 7 → col 16.
        assert_eq!(sanitize_for_display("ab\tc\td"), "ab      c       d");
    }

    #[test]
    fn tab_column_resets_after_newline() {
        // Second line: "cd" ends at column 2 → pad 6.
        assert_eq!(sanitize_for_display("ab\ncd\tx"), "ab\ncd      x");
    }

    #[test]
    fn tab_expansion_is_cjk_width_aware() {
        // "世" occupies 2 columns → tab pads 6 spaces to column 8.
        assert_eq!(sanitize_for_display("世\tx"), "世      x");
    }

    #[test]
    fn tab_column_ignores_stripped_escapes() {
        // The SGR bytes occupy no columns; "ab" ends at column 2 → pad 6.
        assert_eq!(sanitize_for_display("\x1b[31mab\x1b[0m\tc"), "ab      c");
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

    #[test]
    fn dcs_sixel_payload_stripped() {
        // A sixel image: ESC P q <payload> ESC \. Treating ESC P as a
        // two-byte escape used to leak the whole payload as text.
        let s = "before\x1bPq#0;2;0;0;0#0~~@@vv@@~~@@~~$\x1b\\after";
        assert_eq!(sanitize_for_display(s), "beforeafter");
    }

    #[test]
    fn dcs_terminated_by_bel_stripped() {
        let s = "a\x1bPpayload\x07b";
        assert_eq!(sanitize_for_display(s), "ab");
    }

    #[test]
    fn sos_pm_apc_payloads_stripped() {
        // SOS = ESC X, PM = ESC ^, APC = ESC _ (tmux passthrough,
        // kitty graphics). All must swallow their payload up to ST.
        let s = "1\x1bXsos-payload\x1b\\2\x1b^pm-payload\x1b\\3\x1b_Ga=T,f=100\x1b\\4";
        assert_eq!(sanitize_for_display(s), "1234");
    }

    #[test]
    fn tmux_passthrough_wrapping_sixel_stripped() {
        // tmux wraps inner escapes as ESC _ … ESC \ (APC) or via DCS
        // with doubled ESCs; either way none of it may reach the text.
        let s = "ok\x1bPtmux;\x1b\x1bPq#0;2;97;97;97-\x1b\x1b\\\x1b\\done";
        let out = sanitize_for_display(s);
        assert!(!out.contains('#'), "payload leaked: {out:?}");
        assert!(out.starts_with("ok"), "leading text lost: {out:?}");
        assert!(out.ends_with("done"), "trailing text lost: {out:?}");
    }

    #[test]
    fn unterminated_dcs_consumes_to_eof() {
        // Truncated payload (stream cut mid-image) — swallow the rest
        // rather than dumping half a sixel into the transcript.
        let s = "safe\x1bPq#0;2;0;0;0#0~~@@vv";
        assert_eq!(sanitize_for_display(s), "safe");
    }

    #[test]
    fn eight_bit_csi_params_stripped() {
        // U+009B is the single-char C1 form of ESC [. Its parameter
        // bytes must be consumed, not passed through as text.
        let s = "a\u{9b}31mred-param-eaten";
        assert_eq!(sanitize_for_display(s), "ared-param-eaten");
        let s2 = "x\u{9b}2Ky";
        assert_eq!(sanitize_for_display(s2), "xy");
    }
}
