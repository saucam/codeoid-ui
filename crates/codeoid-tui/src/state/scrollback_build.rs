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
