//! Animation primitives. Keep the spinner logic here so the renderer and
//! the status bar share a single source of truth.
//!
//! The app reducer bumps [`AppState::anim_tick`](crate::state::AppState) on
//! each `Tick` event (100 ms). Callers translate a tick into a frame by
//! [`BRAILLE`] indexing and pair it with [`verb_phrase`] to get a
//! Claude-code-style "working" prefix like `⠏ Thinking…`.

/// 10-frame Braille dot spinner — same glyph set as Claude Code / npm CLI.
pub const BRAILLE: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Expressive verb rotation. Cycle slowly (every 30 ticks ≈ 3 s) so the TUI
/// feels alive without fighting for the user's attention.
const VERBS: &[&str] = &[
    "Thinking",
    "Cogitating",
    "Deliberating",
    "Pondering",
    "Unspooling",
    "Reasoning",
    "Weaving",
    "Synthesising",
    "Crunching",
    "Assembling",
];

/// Pick a verb based on a seed (e.g. message id hash) so sibling messages
/// don't all say "Thinking" — each tool call gets a stable verb.
#[must_use]
pub fn verb_phrase(seed: u64, tick: u64) -> &'static str {
    // Rotate slowly — one verb change every ~3 seconds at 100 ms ticks.
    let idx = (seed.wrapping_add(tick / 30)) as usize % VERBS.len();
    VERBS[idx]
}

#[derive(Debug, Clone, Copy)]
pub struct SpinnerFrame(pub &'static str);

impl SpinnerFrame {
    #[must_use]
    pub fn for_tick(tick: u64) -> Self {
        Self(BRAILLE[(tick as usize) % BRAILLE.len()])
    }

    #[must_use]
    pub fn glyph(self) -> &'static str {
        self.0
    }
}

/// Hash helper — converts a string id into a stable u64 seed for verb
/// selection. `std::hash` with `DefaultHasher` is overkill; a quick FNV-1a
/// is plenty and keeps this crate free of `std::hash` noise.
#[must_use]
pub fn seed_from(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}
