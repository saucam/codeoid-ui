//! Frame-to-frame cache of the assembled scrollback `Vec<Line>`.
//!
//! The per-message [`crate::state::RenderCache`] still memoizes individual
//! message renders. This builds on top: it caches the *final assembled
//! buffer* — every visible message's lines concatenated, plus the
//! `total_rendered_rows` count used for scroll math — keyed by
//! `(session_id, width, session_epoch)`.
//!
//! When the user is just typing in the prompt or looking at an idle
//! transcript, the key is unchanged frame-to-frame, so [`Self::matches`]
//! returns true and the renderer skips the entire walk over messages,
//! the per-message cache lookups, and the `total_rendered_rows` re-walk.
//! Only when the focused session changes, the terminal resizes, or a
//! message in the focused session mutates does the cache miss and we
//! rebuild.

use ratatui::text::Line;

#[derive(Default)]
pub struct ScrollbackBuild {
    /// Focused session id at last build. `None` = never built.
    pub session_id: Option<String>,
    /// Inner viewport width (post-border) at last build.
    pub width: u16,
    /// `MessageStore::epoch_of_session` value at last build.
    pub epoch: u64,
    /// Assembled lines including per-message separators.
    pub lines: Vec<Line<'static>>,
    /// Pre-computed `total_rendered_rows(&lines, width)`, so scroll math
    /// reuses it without a second O(N) walk.
    pub total_rendered_rows: usize,
    /// Prefix sum of wrapped rows: `row_offsets[i]` is the number of
    /// screen rows occupied by logical lines `0..i`. Length is
    /// `lines.len() + 1`, so `row_offsets.last() == total_rendered_rows`.
    /// Lets the renderer binary-search the few logical lines that
    /// intersect the viewport and hand ratatui only that slice — each
    /// frame (and each scroll tick) becomes O(viewport), not O(transcript).
    /// See [`visible_window`].
    pub row_offsets: Vec<usize>,
    /// Whether any message in the last build had animating content (running
    /// tool spinners). Stored so the next frame can skip the O(N) scan when
    /// this is `false` — idle sessions get an O(1) cache check.
    pub has_animating: bool,
}

/// Pick the logical-line slice that covers the viewport.
///
/// `row_offsets` is the prefix sum from [`ScrollbackBuild::row_offsets`]
/// (length `lines + 1`). `y` is the topmost visible wrapped row;
/// `viewport_rows` is the viewport height. Returns
/// `(first_line, intra_offset, last_line_exclusive)` such that rendering
/// `lines[first_line..last_line_exclusive]` scrolled down by `intra_offset`
/// rows shows exactly the window `[y, y + viewport_rows)`.
///
/// O(log lines) — two binary searches, no walk over the buffer.
#[must_use]
pub fn visible_window(
    row_offsets: &[usize],
    y: usize,
    viewport_rows: usize,
) -> (usize, usize, usize) {
    let n = row_offsets.len().saturating_sub(1); // number of logical lines
    if n == 0 {
        return (0, 0, 0);
    }
    let total = row_offsets[n];
    // Clamp the top row into range so the searches can't run off the end.
    let y = y.min(total.saturating_sub(1));
    // First logical line whose row range contains `y`: the largest `i`
    // with `row_offsets[i] <= y`.
    let first = row_offsets
        .partition_point(|&o| o <= y)
        .saturating_sub(1)
        .min(n - 1);
    let intra = y - row_offsets[first];
    // Last (exclusive): the first line that starts at or after the bottom
    // of the window, so the slice fully covers `[y, y + viewport_rows)`.
    let bottom = y.saturating_add(viewport_rows);
    let last = row_offsets
        .partition_point(|&o| o < bottom)
        .clamp(first + 1, n);
    (first, intra, last)
}

impl ScrollbackBuild {
    /// Cache hit when the same session, the same width, and the same
    /// session epoch — anything else and the assembled buffer might be
    /// stale.
    #[must_use]
    pub fn matches(&self, session_id: &str, width: u16, epoch: u64) -> bool {
        self.width == width
            && self.epoch == epoch
            && self.session_id.as_deref().is_some_and(|s| s == session_id)
    }

    /// Drop the cached build. Used when a global toggle (e.g.
    /// verbose-tool-output) changes how messages render at the same
    /// width + epoch — the cache is technically still keyed correctly
    /// but the stored `lines` are now stale.
    pub fn clear(&mut self) {
        self.session_id = None;
        self.lines.clear();
        self.total_rendered_rows = 0;
        self.row_offsets.clear();
        self.has_animating = false;
    }
}

impl std::fmt::Debug for ScrollbackBuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollbackBuild")
            .field("session_id", &self.session_id)
            .field("width", &self.width)
            .field("epoch", &self.epoch)
            .field("lines", &self.lines.len())
            .field("total_rendered_rows", &self.total_rendered_rows)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::visible_window;

    // Three logical lines wrapping to 2, 3, and 4 rows → total 9 rows.
    // row_offsets = [0, 2, 5, 9]:  line0=rows 0..2, line1=2..5, line2=5..9.
    const OFFSETS: &[usize] = &[0, 2, 5, 9];

    #[test]
    fn window_in_the_middle() {
        // Top row 4 (3rd row of line1), viewport 3 → rows [4,7).
        let (first, intra, last) = visible_window(OFFSETS, 4, 3);
        assert_eq!((first, intra, last), (1, 2, 3));
    }

    #[test]
    fn window_at_top() {
        let (first, intra, last) = visible_window(OFFSETS, 0, 3);
        assert_eq!((first, intra), (0, 0));
        // Must cover rows [0,3): lines 0 (0..2) and 1 (2..5).
        assert_eq!(last, 2);
    }

    #[test]
    fn window_at_bottom() {
        // total 9, viewport 4 → top row 5. rows [5,9) = all of line2.
        let (first, intra, last) = visible_window(OFFSETS, 5, 4);
        assert_eq!((first, intra, last), (2, 0, 3));
    }

    #[test]
    fn window_covers_whole_buffer_when_short() {
        // Viewport taller than the content: one line, 2 rows.
        let offsets = &[0usize, 2];
        let (first, intra, last) = visible_window(offsets, 0, 10);
        assert_eq!((first, intra, last), (0, 0, 1));
    }

    #[test]
    fn slice_always_covers_the_window() {
        // Property: for any y, the returned slice spans at least
        // [y, y+viewport) and starts at-or-before y.
        let viewport = 3;
        for y in 0..OFFSETS[OFFSETS.len() - 1] {
            let (first, intra, last) = visible_window(OFFSETS, y, viewport);
            let clamped_y = y.min(OFFSETS[3] - 1);
            assert!(OFFSETS[first] <= clamped_y, "first starts after y");
            assert_eq!(OFFSETS[first] + intra, clamped_y);
            assert!(first < last && last <= 3);
            // Rows available in the slice after skipping intra must fill
            // the viewport (unless we hit the end of the buffer).
            let rows_in_slice = OFFSETS[last] - OFFSETS[first];
            assert!(rows_in_slice >= intra + viewport || last == 3);
        }
    }

    #[test]
    fn empty_offsets_are_safe() {
        assert_eq!(visible_window(&[], 0, 10), (0, 0, 0));
        assert_eq!(visible_window(&[0], 0, 10), (0, 0, 0));
    }
}
