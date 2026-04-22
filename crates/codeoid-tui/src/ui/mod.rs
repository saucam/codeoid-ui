//! Top-level render entry. Splits the terminal into zones and fans out to
//! widget modules.
//!
//! Layout (top-to-bottom):
//!   1. Connection + session tabs       (3 rows)
//!   2. Transcript                       (flex)
//!   3. Working / scroll indicator       (1 row)
//!   4. Prompt editor                    (5 rows)
//!   5. Keybinding hints                 (1 row)

mod hints;
mod modal;
mod prompt;
mod scrollback;
mod tabs;
mod worker;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Frame;

use crate::state::AppState;

pub fn render(frame: &mut Frame<'_>, state: &mut AppState) {
    let area = frame.area();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // 1. tabs
            Constraint::Min(5),    // 2. transcript
            Constraint::Length(1), // 3. worker / scroll indicator
            Constraint::Length(5), // 4. prompt
            Constraint::Length(1), // 5. hints
        ])
        .split(area);

    tabs::render(frame, rows[0], state);
    scrollback::render(frame, rows[1], state);
    worker::render(frame, rows[2], state);
    prompt::render(frame, rows[3], state);
    hints::render(frame, rows[4], state);

    modal::render(frame, state);
}
