//! Per-message styled-line cache.
//!
//! Without caching, every frame re-runs ANSI parsing, markdown parsing,
//! and string allocation for every message in the focused session — at
//! 10 Hz while a tool is animating, this is wasted CPU on a 500-message
//! transcript and shows up as visible sluggishness.
//!
//! Strategy: entries are grouped per session and keyed on
//! `(message_id, version, width)`. Version comes from
//! [`crate::state::messages::MessageStore::version_of`] — bumped on
//! every mutation. Width is the rendered width (changes on terminal
//! resize). On hit, we clone the cached `Vec<Line<'static>>`. The clone
//! still allocates strings, but the parse cost is gone — and parsing
//! markdown / ANSI is the dominant cost.
//!
//! Grouping by session matters: retention runs against ONE session's
//! live message ids ([`RenderCache::retain_session`]), so switching
//! focus between sessions no longer evicts every other session's cached
//! renders (which used to force a full O(N) re-parse on every Tab).
//! Total memory stays bounded because the scrollback build LRU evicts a
//! whole session's entries ([`RenderCache::evict_session`]) once that
//! session falls out of the recently-focused window.
//!
//! Animation-driven content (tool spinners, elapsed-time counters) is
//! NOT cached — render with `skip_cache = true` for those messages.

use std::collections::HashMap;

use ratatui::text::Line;

use super::scrollback_build::wrapped_row_count;

#[derive(Default)]
pub struct RenderCache {
    /// `session_id` → (`message_id` → cached render).
    sessions: HashMap<String, HashMap<String, CachedEntry>>,
}

struct CachedEntry {
    version: u64,
    width: u16,
    lines: Vec<Line<'static>>,
    /// Wrapped-row count per line at `width`, measured once when the
    /// entry is stored. Lets the scrollback build assemble its prefix
    /// sum from cached integers instead of re-measuring every line of
    /// the transcript on each rebuild (the dominant per-frame cost
    /// while a tool was animating). A version bump (content delta)
    /// recomputes only THIS message's counts; a width change (resize)
    /// misses every entry — exactly the required invalidation scope.
    row_counts: Vec<usize>,
}

impl RenderCache {
    /// Fetch cached lines (and their wrapped-row counts at `width`) for
    /// a message, or render-and-store on miss.
    /// `skip_cache = true` always re-renders and never stores — use it
    /// for animated content whose appearance changes per anim_tick.
    pub fn get_or_render<F>(
        &mut self,
        session_id: &str,
        message_id: &str,
        version: u64,
        width: u16,
        skip_cache: bool,
        render_fn: F,
    ) -> (Vec<Line<'static>>, Vec<usize>)
    where
        F: FnOnce() -> Vec<Line<'static>>,
    {
        if skip_cache {
            let lines = render_fn();
            let counts = measure(&lines, width);
            return (lines, counts);
        }

        if let Some(entry) = self
            .sessions
            .get(session_id)
            .and_then(|s| s.get(message_id))
        {
            if entry.version == version && entry.width == width {
                return (entry.lines.clone(), entry.row_counts.clone());
            }
        }

        let lines = render_fn();
        let counts = measure(&lines, width);
        self.sessions
            .entry(session_id.to_string())
            .or_default()
            .insert(
                message_id.to_string(),
                CachedEntry {
                    version,
                    width,
                    lines: lines.clone(),
                    row_counts: counts.clone(),
                },
            );
        (lines, counts)
    }

    /// Drop a single message's entry. Called when a message's rendered
    /// appearance changes for reasons the version doesn't capture
    /// (selection highlight, per-block expand).
    pub fn invalidate(&mut self, session_id: &str, message_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.remove(message_id);
        }
    }

    /// Drop everything. Safe escape hatch on global render-affecting
    /// toggles (e.g. verbose tool output).
    pub fn clear(&mut self) {
        self.sessions.clear();
    }

    /// Discard entries for messages no longer present in ONE session's
    /// transcript. Other sessions' entries are untouched — evicting
    /// them on focus switch is exactly the bug this signature prevents.
    pub fn retain_session(&mut self, session_id: &str, keep: &std::collections::HashSet<String>) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.retain(|id, _| keep.contains(id));
        }
    }

    /// Drop every cached render for a session. Called when the session
    /// falls out of the scrollback-build LRU window (least-recently
    /// focused), keeping total memory bounded.
    pub fn evict_session(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
    }

    /// True when a cached render exists for this (session, message).
    #[cfg(test)]
    #[must_use]
    pub fn contains(&self, session_id: &str, message_id: &str) -> bool {
        self.sessions
            .get(session_id)
            .is_some_and(|s| s.contains_key(message_id))
    }
}

/// Wrapped-row count for every line, with ratatui's own math (see
/// [`wrapped_row_count`]). Runs once per store, never per frame.
fn measure(lines: &[Line<'static>], width: u16) -> Vec<usize> {
    lines.iter().map(|l| wrapped_row_count(l, width)).collect()
}

impl std::fmt::Debug for RenderCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderCache")
            .field("sessions", &self.sessions.len())
            .field(
                "entries",
                &self.sessions.values().map(HashMap::len).sum::<usize>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    fn lines(text: &str) -> Vec<Line<'static>> {
        vec![Line::from(Span::raw(text.to_string()))]
    }

    #[test]
    fn miss_renders_and_stores() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || {
            call_count += 1;
            lines("hello")
        });
        assert_eq!(call_count, 1);
    }

    #[test]
    fn hit_avoids_re_render() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || {
            call_count += 1;
            lines("hello")
        });
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || {
            call_count += 1;
            lines("hello")
        });
        assert_eq!(call_count, 1, "second call should hit cache");
    }

    #[test]
    fn version_bump_invalidates() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || {
            call_count += 1;
            lines("v1")
        });
        let _ = cache.get_or_render("s1", "m1", 2, 80, false, || {
            call_count += 1;
            lines("v2")
        });
        assert_eq!(call_count, 2, "version change must re-render");
    }

    #[test]
    fn width_change_invalidates() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || {
            call_count += 1;
            lines("hello")
        });
        let _ = cache.get_or_render("s1", "m1", 1, 60, false, || {
            call_count += 1;
            lines("hello")
        });
        assert_eq!(call_count, 2, "width change must re-render");
    }

    #[test]
    fn skip_cache_always_renders_and_doesnt_store() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("s1", "m1", 1, 80, true, || {
            call_count += 1;
            lines("anim")
        });
        let _ = cache.get_or_render("s1", "m1", 1, 80, true, || {
            call_count += 1;
            lines("anim")
        });
        // And a subsequent non-skip call should also render — nothing
        // was stored.
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || {
            call_count += 1;
            lines("anim")
        });
        assert_eq!(call_count, 3);
    }

    #[test]
    fn retain_session_drops_missing_ids_in_that_session_only() {
        use std::collections::HashSet;
        let mut cache = RenderCache::default();
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || lines("a"));
        let _ = cache.get_or_render("s1", "m2", 1, 80, false, || lines("b"));
        let _ = cache.get_or_render("s2", "m2", 1, 80, false, || lines("other"));

        let mut keep = HashSet::new();
        keep.insert("m1".to_string());
        cache.retain_session("s1", &keep);

        assert!(cache.contains("s1", "m1"));
        assert!(!cache.contains("s1", "m2"), "m2 pruned from s1");
        assert!(
            cache.contains("s2", "m2"),
            "another session's entries must survive retention"
        );
    }

    #[test]
    fn sessions_are_isolated() {
        // The same message id in two sessions must cache independently.
        let mut cache = RenderCache::default();
        let (a, _) = cache.get_or_render("s1", "m1", 1, 80, false, || lines("from-s1"));
        let (b, _) = cache.get_or_render("s2", "m1", 1, 80, false, || lines("from-s2"));
        assert_ne!(a[0].spans[0].content, b[0].spans[0].content);
        // Both hits afterwards.
        let mut renders = 0;
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || {
            renders += 1;
            lines("x")
        });
        let _ = cache.get_or_render("s2", "m1", 1, 80, false, || {
            renders += 1;
            lines("x")
        });
        assert_eq!(renders, 0);
    }

    #[test]
    fn evict_session_drops_all_its_entries() {
        let mut cache = RenderCache::default();
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || lines("a"));
        let _ = cache.get_or_render("s1", "m2", 1, 80, false, || lines("b"));
        let _ = cache.get_or_render("s2", "m1", 1, 80, false, || lines("c"));

        cache.evict_session("s1");

        assert!(!cache.contains("s1", "m1"));
        assert!(!cache.contains("s1", "m2"));
        assert!(cache.contains("s2", "m1"));
    }

    // ============ wrapped-row-count caching ============

    #[test]
    fn row_counts_reflect_wrapping_at_stored_width() {
        let mut cache = RenderCache::default();
        // 25 chars, no spaces, at width 10 → 3 wrapped rows.
        let (_, counts) = cache.get_or_render("s1", "m1", 1, 10, false, || {
            lines("abcdefghijklmnopqrstuvwxy")
        });
        assert_eq!(counts, vec![3]);
        // Cache hit returns the same counts without re-measuring.
        let (_, counts_again) =
            cache.get_or_render("s1", "m1", 1, 10, false, || panic!("must be a cache hit"));
        assert_eq!(counts_again, vec![3]);
    }

    #[test]
    fn content_delta_recomputes_only_that_message() {
        let mut cache = RenderCache::default();
        let _ = cache.get_or_render("s1", "m1", 1, 10, false, || lines("short"));
        let _ = cache.get_or_render("s1", "m2", 1, 10, false, || lines("short"));

        // m1's version bumps (content delta) → only m1 re-renders and
        // re-measures; m2 stays a pure hit.
        let mut renders = 0;
        let (_, c1) = cache.get_or_render("s1", "m1", 2, 10, false, || {
            renders += 1;
            lines("abcdefghijklmnopqrst") // 20 chars → 2 rows at width 10
        });
        let (_, c2) = cache.get_or_render("s1", "m2", 1, 10, false, || {
            renders += 1;
            lines("short")
        });
        assert_eq!(renders, 1, "only the changed message re-renders");
        assert_eq!(c1, vec![2]);
        assert_eq!(c2, vec![1]);
    }

    #[test]
    fn width_change_recomputes_all_counts() {
        let mut cache = RenderCache::default();
        let _ = cache.get_or_render("s1", "m1", 1, 40, false, || lines("abcdefghijklmnopqrst"));
        let _ = cache.get_or_render("s1", "m2", 1, 40, false, || lines("abcdefghijklmnopqrst"));

        // Resize → every entry misses and counts are re-measured at the
        // new width (20 chars at width 10 → 2 rows).
        let mut renders = 0;
        let (_, c1) = cache.get_or_render("s1", "m1", 1, 10, false, || {
            renders += 1;
            lines("abcdefghijklmnopqrst")
        });
        let (_, c2) = cache.get_or_render("s1", "m2", 1, 10, false, || {
            renders += 1;
            lines("abcdefghijklmnopqrst")
        });
        assert_eq!(renders, 2, "width change must re-render everything");
        assert_eq!(c1, vec![2]);
        assert_eq!(c2, vec![2]);
    }

    #[test]
    fn invalidate_is_session_scoped() {
        let mut cache = RenderCache::default();
        let _ = cache.get_or_render("s1", "m1", 1, 80, false, || lines("a"));
        let _ = cache.get_or_render("s2", "m1", 1, 80, false, || lines("b"));
        cache.invalidate("s1", "m1");
        assert!(!cache.contains("s1", "m1"));
        assert!(cache.contains("s2", "m1"));
    }
}
