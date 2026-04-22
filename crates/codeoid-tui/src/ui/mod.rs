//! Top-level render entry. Splits the terminal into zones and fans out to
//! widget modules.

mod modal;
mod prompt;
mod scrollback;
mod status;
mod tabs;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Frame;

use crate::state::AppState;

pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // session tabs
            Constraint::Min(5),    // scrollback
            Constraint::Length(5), // prompt
            Constraint::Length(1), // status bar
        ])
        .split(area);

    tabs::render(frame, rows[0], state);
    scrollback::render(frame, rows[1], state);
    prompt::render(frame, rows[2], state);
    status::render(frame, rows[3], state);

    modal::render(frame, state);
}
