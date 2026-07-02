//! Frame-to-frame cache of the assembled scrollback `Vec<Line>`.
//!
//! The per-message [`crate::state::RenderCache`] still memoizes individual
//! message renders. This builds on top: it caches the *final assembled
//! buffer* — every visible message's lines concatenated, plus the
//! `total_rendered_rows` count used for scroll math — keyed by
//! `(session_id, width, session_epoch)`.
//!
//! When the user is just typing in the prompt or looking at an idle
//! transcript, the key is unchanged frame-to-frame, so
//! [`ScrollbackBuildCache::matches`] returns true and the renderer skips
//! the entire walk over messages, the per-message cache lookups, and the
//! `total_rendered_rows` re-walk. Only when the terminal resizes or a
//! message in the focused session mutates does the cache miss and we
//! rebuild.
//!
//! Builds are kept for the [`LRU_SESSIONS`] most-recently-focused
//! sessions (not just the focused one), so Tab A→B→A re-renders nothing.
//! When a session falls off the LRU, the renderer also evicts its
//! per-message render-cache entries, keeping total memory bounded.
//!
//! While a tool is animating (spinner / elapsed counter at 10 Hz), the
//! build is NOT rebuilt: [`ScrollbackBuild::splice_message`] re-renders
//! only the animating message's lines and splices them into the cached
//! buffer, reusing the per-line wrapped-row counts for everything else —
//! the per-frame cost is O(animating lines), not O(transcript).

use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

/// How many sessions' assembled builds to keep. Small on purpose:
/// each build holds a full transcript's styled lines, and the tab-flip
/// pattern this serves rarely touches more than a handful of sessions.
pub const LRU_SESSIONS: usize = 4;

/// Wrapped-row count for a single logical line at `width`, using
/// ratatui's own `Paragraph::line_count` with the same `Wrap` config as
/// the transcript renderer. Ratatui wraps each `Line` independently, so
/// per-line counts sum to the whole-buffer count — keeping scroll math
/// byte-for-byte consistent with the rendered layout. This is the ONLY
/// row-count function the scrollback path uses; counts computed here
/// are cached (per message in `RenderCache`, per line in
/// [`ScrollbackBuild::row_counts`]) so it runs on content change, not
/// per frame.
#[must_use]
pub fn wrapped_row_count(line: &Line<'_>, width: u16) -> usize {
    Paragraph::new(vec![line.clone()])
        .wrap(Wrap { trim: false })
        .line_count(width)
}

/// Maps one rendered message to its slice of [`ScrollbackBuild::lines`]
/// (including the trailing blank separator line). Lets the animation
/// path splice a re-rendered message in place without walking or
/// re-measuring the rest of the transcript.
#[derive(Debug, Clone)]
pub struct BuildSegment {
    pub message_id: String,
    /// Index of the message's first line in `lines`.
    pub first_line: usize,
    /// Number of lines, separator included.
    pub line_count: usize,
}

/// One session's assembled scrollback build.
#[derive(Default)]
pub struct ScrollbackBuild {
    /// Inner viewport width (post-border) at build time.
    pub width: u16,
    /// `MessageStore::epoch_of_session` value at build time.
    pub epoch: u64,
    /// Assembled lines including per-message separators.
    pub lines: Vec<Line<'static>>,
    /// Pre-computed `total_rendered_rows(&lines, width)`, so scroll math
    /// reuses it without a second O(N) walk.
    pub total_rendered_rows: usize,
    /// Per-logical-line wrapped-row counts (`wrapped_row_count` of each
    /// entry in `lines` at `width`). Kept so the prefix sum can be
    /// recomputed with integer adds only — no re-wrapping — when a
    /// splice changes one message's shape.
    pub row_counts: Vec<usize>,
    /// Prefix sum of wrapped rows: `row_offsets[i]` is the number of
    /// screen rows occupied by logical lines `0..i`. Length is
    /// `lines.len() + 1`, so `row_offsets.last() == total_rendered_rows`.
    /// Lets the renderer binary-search the few logical lines that
    /// intersect the viewport and hand ratatui only that slice — each
    /// frame (and each scroll tick) becomes O(viewport), not O(transcript).
    /// See [`visible_window`].
    pub row_offsets: Vec<usize>,
    /// Per-message line ranges, in transcript order. Messages that
    /// rendered empty (mid-stream placeholders) have no segment.
    pub segments: Vec<BuildSegment>,
}

impl ScrollbackBuild {
    /// Replace one message's lines in place (animation repaint).
    ///
    /// `new_lines` must include the trailing separator line if the
    /// message renders non-empty — i.e. exactly what the full rebuild
    /// would have appended for this message. No-op if the message has
    /// no segment (it rendered empty at build time; the next epoch bump
    /// will pick it up).
    ///
    /// Cost: O(new lines) for the re-measure, plus an integer prefix-sum
    /// rebuild ONLY when the message's wrapped shape actually changed
    /// (a spinner glyph swap usually doesn't).
    pub fn splice_message(&mut self, message_id: &str, new_lines: Vec<Line<'static>>) {
        let Some(idx) = self
            .segments
            .iter()
            .position(|s| s.message_id == message_id)
        else {
            return;
        };
        let start = self.segments[idx].first_line;
        let old_len = self.segments[idx].line_count;
        let new_len = new_lines.len();

        let new_counts: Vec<usize> = new_lines
            .iter()
            .map(|l| wrapped_row_count(l, self.width))
            .collect();
        let shape_changed = new_counts[..] != self.row_counts[start..start + old_len];

        self.lines.splice(start..start + old_len, new_lines);
        self.row_counts.splice(start..start + old_len, new_counts);
        self.segments[idx].line_count = new_len;

        if new_len > old_len {
            let delta = new_len - old_len;
            for seg in &mut self.segments[idx + 1..] {
                seg.first_line += delta;
            }
        } else if new_len < old_len {
            let delta = old_len - new_len;
            for seg in &mut self.segments[idx + 1..] {
                seg.first_line -= delta;
            }
        }
        if shape_changed {
            self.rebuild_offsets();
        }
    }

    /// Recompute the prefix sum + total from the cached per-line counts.
    /// Integer adds only — no text measurement.
    pub fn rebuild_offsets(&mut self) {
        self.row_offsets.clear();
        self.row_offsets.reserve(self.row_counts.len() + 1);
        let mut acc = 0usize;
        self.row_offsets.push(0);
        for &c in &self.row_counts {
            acc += c;
            self.row_offsets.push(acc);
        }
        self.total_rendered_rows = acc;
    }
}

/// LRU of per-session builds, most-recently-focused first.
#[derive(Default)]
pub struct ScrollbackBuildCache {
    entries: Vec<(String, ScrollbackBuild)>,
}

impl ScrollbackBuildCache {
    /// Cache hit when we hold a build for this session at the same
    /// width and session epoch — anything else and the assembled buffer
    /// might be stale.
    #[must_use]
    pub fn matches(&self, session_id: &str, width: u16, epoch: u64) -> bool {
        self.get(session_id)
            .is_some_and(|b| b.width == width && b.epoch == epoch)
    }

    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<&ScrollbackBuild> {
        self.entries
            .iter()
            .find(|(id, _)| id == session_id)
            .map(|(_, b)| b)
    }

    #[must_use]
    pub fn get_mut(&mut self, session_id: &str) -> Option<&mut ScrollbackBuild> {
        self.entries
            .iter_mut()
            .find(|(id, _)| id == session_id)
            .map(|(_, b)| b)
    }

    /// Mark a session as most-recently-focused without rebuilding.
    pub fn touch(&mut self, session_id: &str) {
        if let Some(pos) = self.entries.iter().position(|(id, _)| id == session_id) {
            if pos > 0 {
                let entry = self.entries.remove(pos);
                self.entries.insert(0, entry);
            }
        }
    }

    /// Insert (or replace) a session's build at the front of the LRU.
    /// Returns the session id evicted from the tail, if the window
    /// overflowed — the caller must drop that session's render-cache
    /// entries too, or memory grows unbounded across many sessions.
    #[must_use = "evicted session's render-cache entries must be dropped by the caller"]
    pub fn insert(&mut self, session_id: String, build: ScrollbackBuild) -> Option<String> {
        if let Some(pos) = self.entries.iter().position(|(id, _)| id == &session_id) {
            self.entries.remove(pos);
        }
        self.entries.insert(0, (session_id, build));
        if self.entries.len() > LRU_SESSIONS {
            self.entries.pop().map(|(id, _)| id)
        } else {
            None
        }
    }

    /// Drop every cached build. Used when a global toggle (e.g.
    /// verbose-tool-output) changes how messages render at the same
    /// width + epoch — the keys are technically still correct but every
    /// stored `lines` is now stale.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
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

impl std::fmt::Debug for ScrollbackBuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollbackBuild")
            .field("width", &self.width)
            .field("epoch", &self.epoch)
            .field("lines", &self.lines.len())
            .field("total_rendered_rows", &self.total_rendered_rows)
            .finish()
    }
}

impl std::fmt::Debug for ScrollbackBuildCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollbackBuildCache")
            .field(
                "sessions",
                &self
                    .entries
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{visible_window, ScrollbackBuild, ScrollbackBuildCache, LRU_SESSIONS};

    // Three logical lines wrapping to 2, 3, and 4 rows → total 9 rows.
    // row_offsets = [0, 2, 5, 9]:  line0=rows 0..2, line1=2..5, line2=5..9.
    const OFFSETS: &[usize] = &[0, 2, 5, 9];

    fn mk_build(width: u16, epoch: u64) -> ScrollbackBuild {
        ScrollbackBuild {
            width,
            epoch,
            ..ScrollbackBuild::default()
        }
    }

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

    // ============ LRU cache ============

    #[test]
    fn cache_holds_builds_for_multiple_sessions() {
        let mut cache = ScrollbackBuildCache::default();
        assert!(cache.insert("a".into(), mk_build(80, 1)).is_none());
        assert!(cache.insert("b".into(), mk_build(80, 7)).is_none());
        // Focusing B did not evict A's build: A is still a hit.
        assert!(cache.matches("a", 80, 1));
        assert!(cache.matches("b", 80, 7));
    }

    #[test]
    fn matches_requires_same_width_and_epoch() {
        let mut cache = ScrollbackBuildCache::default();
        let _ = cache.insert("a".into(), mk_build(80, 1));
        assert!(!cache.matches("a", 79, 1), "resize must miss");
        assert!(!cache.matches("a", 80, 2), "epoch bump must miss");
        assert!(!cache.matches("zzz", 80, 1), "unknown session must miss");
    }

    #[test]
    fn eviction_beyond_lru_window_returns_oldest() {
        let mut cache = ScrollbackBuildCache::default();
        for i in 0..LRU_SESSIONS {
            assert!(cache.insert(format!("s{i}"), mk_build(80, 1)).is_none());
        }
        // One more → the least-recently-inserted (s0) falls off.
        let evicted = cache.insert("extra".into(), mk_build(80, 1));
        assert_eq!(evicted.as_deref(), Some("s0"));
        assert!(!cache.matches("s0", 80, 1));
        assert!(cache.matches("s1", 80, 1));
        assert!(cache.matches("extra", 80, 1));
    }

    #[test]
    fn touch_protects_a_session_from_eviction() {
        let mut cache = ScrollbackBuildCache::default();
        for i in 0..LRU_SESSIONS {
            let _ = cache.insert(format!("s{i}"), mk_build(80, 1));
        }
        // Re-focus s0 (oldest) without rebuilding…
        cache.touch("s0");
        // …then overflow: s1 is now the least-recently-focused.
        let evicted = cache.insert("extra".into(), mk_build(80, 1));
        assert_eq!(evicted.as_deref(), Some("s1"));
        assert!(cache.matches("s0", 80, 1));
    }

    #[test]
    fn reinsert_replaces_without_eviction() {
        let mut cache = ScrollbackBuildCache::default();
        for i in 0..LRU_SESSIONS {
            let _ = cache.insert(format!("s{i}"), mk_build(80, 1));
        }
        // Rebuilding an already-cached session must not evict anyone.
        let evicted = cache.insert("s2".into(), mk_build(80, 9));
        assert!(evicted.is_none());
        assert!(cache.matches("s2", 80, 9));
        assert!(cache.matches("s0", 80, 1));
    }

    #[test]
    fn clear_drops_everything() {
        let mut cache = ScrollbackBuildCache::default();
        let _ = cache.insert("a".into(), mk_build(80, 1));
        cache.clear();
        assert!(!cache.matches("a", 80, 1));
        assert!(cache.get("a").is_none());
    }

    // ============ splice_message ============

    use super::BuildSegment;
    use ratatui::text::{Line, Span};

    fn text_line(s: &str) -> Line<'static> {
        Line::from(Span::raw(s.to_string()))
    }

    /// Two messages: m1 = 1 content line + separator, m2 = 2 content
    /// lines + separator. Width 10.
    fn mk_spliceable() -> ScrollbackBuild {
        let mut b = ScrollbackBuild {
            width: 10,
            epoch: 1,
            lines: vec![
                text_line("m1 body"),
                text_line(""),
                text_line("m2 first"),
                text_line("m2 second"),
                text_line(""),
            ],
            total_rendered_rows: 0,
            row_counts: vec![1, 1, 1, 1, 1],
            row_offsets: Vec::new(),
            segments: vec![
                BuildSegment {
                    message_id: "m1".into(),
                    first_line: 0,
                    line_count: 2,
                },
                BuildSegment {
                    message_id: "m2".into(),
                    first_line: 2,
                    line_count: 3,
                },
            ],
        };
        b.rebuild_offsets();
        b
    }

    fn line_text(l: &Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn splice_same_shape_replaces_lines_and_keeps_offsets() {
        let mut b = mk_spliceable();
        let offsets_before = b.row_offsets.clone();
        b.splice_message("m1", vec![text_line("m1 v2"), text_line("")]);

        assert_eq!(line_text(&b.lines[0]), "m1 v2");
        assert_eq!(line_text(&b.lines[2]), "m2 first", "m2 untouched");
        assert_eq!(b.row_offsets, offsets_before, "same shape → same offsets");
        assert_eq!(b.total_rendered_rows, 5);
    }

    #[test]
    fn splice_growing_message_shifts_following_segments() {
        let mut b = mk_spliceable();
        b.splice_message(
            "m1",
            vec![text_line("m1 a"), text_line("m1 b"), text_line("")],
        );

        assert_eq!(b.lines.len(), 6);
        assert_eq!(b.segments[0].line_count, 3);
        assert_eq!(b.segments[1].first_line, 3, "m2 shifted down by 1");
        assert_eq!(b.total_rendered_rows, 6);
        assert_eq!(b.row_offsets.len(), 7);
        assert_eq!(line_text(&b.lines[3]), "m2 first");
    }

    #[test]
    fn splice_shrinking_message_shifts_following_segments_up() {
        let mut b = mk_spliceable();
        b.splice_message("m2", vec![text_line("m2 only"), text_line("")]);

        assert_eq!(b.lines.len(), 4);
        assert_eq!(b.segments[1].line_count, 2);
        assert_eq!(b.total_rendered_rows, 4);
        assert_eq!(line_text(&b.lines[2]), "m2 only");
    }

    #[test]
    fn splice_wrap_count_change_rebuilds_offsets() {
        let mut b = mk_spliceable();
        // Width 10: a 25-char unbroken token wraps to 3 rows.
        b.splice_message(
            "m1",
            vec![text_line("abcdefghijklmnopqrstuvwxy"), text_line("")],
        );

        assert_eq!(b.row_counts[0], 3);
        assert_eq!(b.total_rendered_rows, 7);
        assert_eq!(b.row_offsets, vec![0, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn splice_unknown_message_is_a_noop() {
        let mut b = mk_spliceable();
        let before = b.lines.len();
        b.splice_message("nope", vec![text_line("x")]);
        assert_eq!(b.lines.len(), before);
        assert_eq!(b.total_rendered_rows, 5);
    }
}
