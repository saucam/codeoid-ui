//! Data-to-visual helpers. Pure functions that turn protocol types into
//! `ratatui::text::Line`s — kept separate from the widget layout code in
//! `ui/` so they're trivial to unit test.
//!
//! Intentionally empty for now; widgets render inline in `ui/*`. As the
//! markdown renderer and diff highlighter come online, they'll live here.

#![allow(dead_code)]
