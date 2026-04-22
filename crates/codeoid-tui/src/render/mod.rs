//! Data-to-visual helpers. Pure functions that turn protocol types into
//! `ratatui::text::Line`s. Kept separate from the widget-layout code in
//! `ui/` so they stay trivially unit-testable.

pub mod markdown;
pub mod spinner;
pub mod tool;

pub use markdown::render_markdown_block;
pub use spinner::{verb_phrase, SpinnerFrame};
pub use tool::render_tool_block;
