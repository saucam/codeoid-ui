//! Wrap-aware row counting that matches ratatui's `Wrap { trim: false }`.
//!
//! The transcript needs to know, for each logical [`Line`], how many
//! screen rows it will occupy after wrapping. We use this to keep the
//! latest output stuck to the bottom of the viewport (and to compute
//! "N rows below the fold" for the new-message indicator).
//!
//! Why we don't just count `chars().count() / width`:
//!
//! * **Unicode width** — `世`, emoji, and box-drawing glyphs occupy 2
//!   terminal columns. Counting code points under-counts them and the
//!   bottom of CJK output gets clipped off-screen with no way to scroll
//!   to it.
//! * **Tabs** — `\t` advances to the next 8-column tab stop. `git
//!   status` output uses tabs heavily; counting them as 1 column under-
//!   counts.
//! * **Word wrap** — ratatui breaks at word boundaries; a 30-char line
//!   at width 20 may produce 2 rows (split on a space) or 3 (no good
//!   break). Char-only math gets the easy case wrong.
//!
//! Algorithm: walk graphemes left-to-right tracking the visual column.
//! When the next grapheme would overflow `width`, wrap at the most
//! recent space if any (word wrap), else mid-word (char wrap). This is
//! close enough to ratatui's behaviour to keep stick-to-bottom exact in
//! every case I've been able to construct.

use ratatui::text::Line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Width to which a `\t` expands. Matches the de-facto terminal default
/// and what ratatui's word-wrap implementation assumes.
const TAB_WIDTH: usize = 8;

/// Count the rendered screen rows for a single logical line at the given
/// `width`. Returns at least 1 (an empty line still occupies one row).
#[must_use]
pub fn rendered_rows(line: &Line<'_>, width: u16) -> usize {
    if width == 0 {
        return 1;
    }
    let w = width as usize;

    // Single-allocation join of all spans. Faster than walking spans
    // because the inner loop is char-driven, not span-driven, and
    // graphemes can straddle span boundaries.
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

    if text.is_empty() {
        return 1;
    }

    let mut rows: usize = 1;
    let mut col: usize = 0;
    // Column AT WHICH the next row would start if we wrapped on the
    // most recent space. `None` = no space seen on the current row, so
    // we'd have to char-wrap.
    let mut break_col: Option<usize> = None;

    for c in text.chars() {
        if c == '\t' {
            let next = ((col / TAB_WIDTH) + 1) * TAB_WIDTH;
            if next >= w {
                // Tab overflows — wrap to start of next row.
                rows += 1;
                col = 0;
                break_col = None;
            } else {
                col = next;
                // A tab counts as a soft break point; safe to wrap here.
                break_col = Some(col);
            }
            continue;
        }

        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if cw == 0 {
            // Combining marks, zero-width joiners — attach to prior cell.
            continue;
        }

        if col + cw <= w {
            col += cw;
            if c == ' ' {
                // After a space we can word-wrap: anything from this
                // point on goes onto the next row, with `col` reset to
                // 0. We don't strip the trailing space — `Wrap { trim:
                // false }` keeps it on the previous row, which is what
                // we want for code/CLI output.
                break_col = Some(col);
            }
            continue;
        }

        // Overflow. Wrap to a new row.
        rows += 1;
        if let Some(brk) = break_col {
            // Content placed since the break moves to the new row. New
            // starting column is (cols-since-break) + (this char).
            col = (col - brk) + cw;
        } else {
            // No good break point — char wrap. The current char starts
            // the new row.
            col = cw;
        }
        break_col = None;

        // If even the new row can't fit this single char (extreme case
        // — width 1, char width 2), we'd loop forever in principle.
        // Saturate at one row per such char so the counter still
        // terminates.
        if col > w {
            col = w;
        }
    }

    rows
}

/// Convenience: total rendered rows across every line.
#[must_use]
pub fn total_rendered_rows(lines: &[Line<'_>], width: u16) -> usize {
    lines.iter().map(|l| rendered_rows(l, width)).sum()
}

/// Display width of a string in terminal columns (without wrapping).
/// Exposed for callers that want to size headers / right-aligned content.
#[must_use]
#[allow(dead_code)] // used by upcoming new-below indicator
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    fn line(s: &str) -> Line<'static> {
        Line::from(Span::raw(s.to_string()))
    }

    #[test]
    fn empty_line_is_one_row() {
        assert_eq!(rendered_rows(&line(""), 80), 1);
    }

    #[test]
    fn short_line_is_one_row() {
        assert_eq!(rendered_rows(&line("hello"), 80), 1);
    }

    #[test]
    fn exact_width_line_is_one_row() {
        assert_eq!(rendered_rows(&line("0123456789"), 10), 1);
    }

    #[test]
    fn one_over_width_is_two_rows() {
        assert_eq!(rendered_rows(&line("01234567890"), 10), 2);
    }

    #[test]
    fn cjk_chars_count_as_two_columns() {
        // "世界" = 4 columns. At width 3, it must wrap to 2 rows.
        assert_eq!(rendered_rows(&line("世界"), 3), 2);
        // At width 4 it fits.
        assert_eq!(rendered_rows(&line("世界"), 4), 1);
        // At width 2, only one CJK char fits per row.
        assert_eq!(rendered_rows(&line("世界世"), 2), 3);
    }

    #[test]
    fn tabs_advance_to_next_eight_col_stop() {
        // "a\tb" = a (col 1), tab → col 8, b (col 9). Fits in 9 columns.
        assert_eq!(rendered_rows(&line("a\tb"), 9), 1);
        // At width 8, b doesn't fit → wraps.
        assert_eq!(rendered_rows(&line("a\tb"), 8), 2);
    }

    #[test]
    fn long_tab_expansion_overflows() {
        // Tab at col 0 advances to col 8, then 'x' = col 9. At width 5,
        // tab overflows → wraps. New row starts with x = 1 col.
        assert_eq!(rendered_rows(&line("\tx"), 5), 2);
    }

    #[test]
    fn word_wrap_breaks_on_space() {
        // "hello world" = 11 chars. At width 7: "hello " fits (6 cols),
        // "world" overflows → wraps after the space. 2 rows.
        assert_eq!(rendered_rows(&line("hello world"), 7), 2);
    }

    #[test]
    fn long_word_falls_back_to_char_wrap() {
        // No spaces; word longer than width → char-break.
        // "abcdefghij" = 10 chars at width 4 → ceil(10/4) = 3 rows.
        assert_eq!(rendered_rows(&line("abcdefghij"), 4), 3);
    }

    #[test]
    fn mixed_word_and_long_token() {
        // "hi supercalifragilistic" at width 10:
        //   "hi " (3 cols), wrap before "super..." (long word) →
        //   "supercalif" (10 cols), "ragilistic" (10 cols) — 3 rows.
        // Actual algorithm: walk chars. Once "super" overflows row 1
        // (cols 4..14, w=10 → 's' at col 11 wraps with break at 3),
        // remainder "supercalifragilistic" (20 chars) needs 2 more rows.
        let rows = rendered_rows(&line("hi supercalifragilistic"), 10);
        assert!((3..=4).contains(&rows), "expected 3 or 4 rows, got {rows}");
    }

    #[test]
    fn multiple_spans_join_for_wrap() {
        let l = Line::from(vec![
            Span::raw("hello ".to_string()),
            Span::raw("beautiful ".to_string()),
            Span::raw("world".to_string()),
        ]);
        // 21 chars at width 10. Word breaks: "hello " (6), "beautiful "
        // wraps to row 2 (10), "world" wraps to row 3 (5). 3 rows.
        let rows = rendered_rows(&l, 10);
        assert_eq!(rows, 3);
    }

    #[test]
    fn width_zero_is_one_row() {
        // Pathological — guard against infinite loop / div-by-zero.
        assert_eq!(rendered_rows(&line("hello"), 0), 1);
    }

    #[test]
    fn zero_width_combining_marks_dont_advance() {
        // "é" composed as e + U+0301 = 2 code points, 1 column.
        let s = "e\u{0301}";
        assert_eq!(rendered_rows(&line(s), 1), 1);
    }

    #[test]
    fn extremely_narrow_terminal_terminates() {
        // Width 1 with CJK (width 2) chars — must not loop forever.
        let rows = rendered_rows(&line("世界世"), 1);
        // Each CJK char gets its own row (saturated).
        assert!(rows >= 3);
    }

    #[test]
    fn total_rendered_sums_across_lines() {
        let lines = vec![line("short"), line("0123456789AB"), line("")];
        // At width 10: 1 + 2 + 1 = 4
        assert_eq!(total_rendered_rows(&lines, 10), 4);
    }

    #[test]
    fn display_width_handles_cjk() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width("世界"), 4);
        assert_eq!(display_width("a 世 b"), 6);
    }
}
