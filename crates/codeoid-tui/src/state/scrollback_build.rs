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

use ratatui::text::Line;

/// How many sessions' assembled builds to keep. Small on purpose:
/// each build holds a full transcript's styled lines, and the tab-flip
/// pattern this serves rarely touches more than a handful of sessions.
pub const LRU_SESSIONS: usize = 4;

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
    /// Prefix sum of wrapped rows: `row_offsets[i]` is the number of
    /// screen rows occupied by logical lines `0..i`. Length is
    /// `lines.len() + 1`, so `row_offsets.last() == total_rendered_rows`.
    /// Lets the renderer binary-search the few logical lines that
    /// intersect the viewport and hand ratatui only that slice — each
    /// frame (and each scroll tick) becomes O(viewport), not O(transcript).
    /// See [`visible_window`].
    pub row_offsets: Vec<usize>,
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
}
