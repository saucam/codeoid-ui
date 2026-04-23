//! Per-message styled-line cache.
//!
//! Without caching, every frame re-runs ANSI parsing, markdown parsing,
//! and string allocation for every message in the focused session — at
//! 10 Hz while a tool is animating, this is wasted CPU on a 500-message
//! transcript and shows up as visible sluggishness.
//!
//! Strategy: key on `(message_id, version, width)`. Version comes from
//! [`crate::state::messages::MessageStore::version_of`] — bumped on
//! every mutation. Width is the rendered width (changes on terminal
//! resize). On hit, we clone the cached `Vec<Line<'static>>`. The clone
//! still allocates strings, but the parse cost is gone — and parsing
//! markdown / ANSI is the dominant cost.
//!
//! Animation-driven content (tool spinners, elapsed-time counters) is
//! NOT cached — render with `skip_cache = true` for those messages.

use std::collections::HashMap;

use ratatui::text::Line;

#[derive(Default)]
pub struct RenderCache {
    entries: HashMap<String, CachedEntry>,
}

struct CachedEntry {
    version: u64,
    width: u16,
    lines: Vec<Line<'static>>,
}

impl RenderCache {
    /// Fetch cached lines for a message, or render-and-store on miss.
    /// `skip_cache = true` always re-renders and never stores — use it
    /// for animated content whose appearance changes per anim_tick.
    pub fn get_or_render<F>(
        &mut self,
        message_id: &str,
        version: u64,
        width: u16,
        skip_cache: bool,
        render_fn: F,
    ) -> Vec<Line<'static>>
    where
        F: FnOnce() -> Vec<Line<'static>>,
    {
        if skip_cache {
            return render_fn();
        }

        if let Some(entry) = self.entries.get(message_id) {
            if entry.version == version && entry.width == width {
                return entry.lines.clone();
            }
        }

        let lines = render_fn();
        self.entries.insert(
            message_id.to_string(),
            CachedEntry {
                version,
                width,
                lines: lines.clone(),
            },
        );
        lines
    }

    /// Drop a single message's entry. Called when a message is removed
    /// (rare today; here for completeness).
    #[allow(dead_code)]
    pub fn invalidate(&mut self, message_id: &str) {
        self.entries.remove(message_id);
    }

    /// Drop everything. Safe escape hatch on session reset.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Discard entries for messages no longer present. Keeps memory
    /// bounded as scrollback grows or sessions get pruned.
    pub fn retain_only(&mut self, keep: &std::collections::HashSet<String>) {
        self.entries.retain(|id, _| keep.contains(id));
    }
}

impl std::fmt::Debug for RenderCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderCache")
            .field("entries", &self.entries.len())
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
        let _ = cache.get_or_render("m1", 1, 80, false, || {
            call_count += 1;
            lines("hello")
        });
        assert_eq!(call_count, 1);
    }

    #[test]
    fn hit_avoids_re_render() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("m1", 1, 80, false, || {
            call_count += 1;
            lines("hello")
        });
        let _ = cache.get_or_render("m1", 1, 80, false, || {
            call_count += 1;
            lines("hello")
        });
        assert_eq!(call_count, 1, "second call should hit cache");
    }

    #[test]
    fn version_bump_invalidates() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("m1", 1, 80, false, || {
            call_count += 1;
            lines("v1")
        });
        let _ = cache.get_or_render("m1", 2, 80, false, || {
            call_count += 1;
            lines("v2")
        });
        assert_eq!(call_count, 2, "version change must re-render");
    }

    #[test]
    fn width_change_invalidates() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("m1", 1, 80, false, || {
            call_count += 1;
            lines("hello")
        });
        let _ = cache.get_or_render("m1", 1, 60, false, || {
            call_count += 1;
            lines("hello")
        });
        assert_eq!(call_count, 2, "width change must re-render");
    }

    #[test]
    fn skip_cache_always_renders_and_doesnt_store() {
        let mut cache = RenderCache::default();
        let mut call_count = 0;
        let _ = cache.get_or_render("m1", 1, 80, true, || {
            call_count += 1;
            lines("anim")
        });
        let _ = cache.get_or_render("m1", 1, 80, true, || {
            call_count += 1;
            lines("anim")
        });
        // And a subsequent non-skip call should also render — nothing
        // was stored.
        let _ = cache.get_or_render("m1", 1, 80, false, || {
            call_count += 1;
            lines("anim")
        });
        assert_eq!(call_count, 3);
    }

    #[test]
    fn retain_only_drops_missing_ids() {
        use std::collections::HashSet;
        let mut cache = RenderCache::default();
        let _ = cache.get_or_render("m1", 1, 80, false, || lines("a"));
        let _ = cache.get_or_render("m2", 1, 80, false, || lines("b"));
        let _ = cache.get_or_render("m3", 1, 80, false, || lines("c"));

        let mut keep = HashSet::new();
        keep.insert("m1".to_string());
        keep.insert("m3".to_string());
        cache.retain_only(&keep);

        // m2 dropped → re-rendering it triggers the closure again.
        let mut renders = 0;
        let _ = cache.get_or_render("m2", 1, 80, false, || {
            renders += 1;
            lines("b")
        });
        assert_eq!(renders, 1);
    }
}
