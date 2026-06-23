//! Top-level render entry. Splits the terminal into zones and fans out to
//! widget modules.
//!
//! Layout (top-to-bottom):
//!   1. Connection + session tabs       (3 rows)
//!   2. Transcript                       (flex)
//!   3. Working / scroll indicator       (1 row)
//!   4. Prompt editor                    (5 rows)
//!   5. Keybinding hints                 (1 row)

mod approval;
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

    // A high-visibility approval banner is inserted between the transcript
    // and the worker row, but only while the focused session has a tool
    // awaiting confirmation. Otherwise the layout is exactly as before.
    let show_approval = approval::is_pending(state);

    let mut constraints = vec![
        Constraint::Length(3), // tabs
        Constraint::Min(5),    // transcript
    ];
    if show_approval {
        constraints.push(Constraint::Length(approval::HEIGHT)); // approval banner
    }
    constraints.push(Constraint::Length(1)); // worker / scroll indicator
    constraints.push(Constraint::Length(5)); // prompt
    constraints.push(Constraint::Length(1)); // hints

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut i = 0;
    tabs::render(frame, rows[i], state);
    i += 1;
    scrollback::render(frame, rows[i], state);
    i += 1;
    if show_approval {
        approval::render(frame, rows[i], state);
        i += 1;
    }
    worker::render(frame, rows[i], state);
    i += 1;
    prompt::render(frame, rows[i], state);
    i += 1;
    hints::render(frame, rows[i], state);

    modal::render(frame, state);
}
