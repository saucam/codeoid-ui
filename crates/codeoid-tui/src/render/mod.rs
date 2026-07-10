//! Data-to-visual helpers. Pure functions that turn protocol types into
//! `ratatui::text::Line`s. Kept separate from the widget-layout code in
//! `ui/` so they stay trivially unit-testable.

pub mod ansi;
pub mod markdown;
pub mod parts;
pub mod sanitize;
pub mod spinner;
pub mod tool;

pub use ansi::parse_ansi;
pub use markdown::render_markdown_block;
pub use parts::{has_rich_parts, render_parts};
pub use sanitize::sanitize_for_display;
pub use spinner::{verb_phrase, SpinnerFrame};
pub use tool::render_tool_block;
