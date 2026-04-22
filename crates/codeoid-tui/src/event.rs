//! Unified event enum — the single input to the app reducer.
//!
//! Keyboard, terminal resize, daemon pushes, and our own tick all land here.
//! The event loop merges crossterm's stream with the daemon's stream and
//! drives the reducer from one place.

use codeoid_client::StreamEvent;
use crossterm::event::Event as CtEvent;

/// Everything the app reducer can observe.
#[derive(Debug)]
pub enum AppEvent {
    /// Raw terminal input (keys, resize, mouse).
    Terminal(CtEvent),
    /// Network event from the daemon stream.
    Net(StreamEvent),
    /// Periodic tick — drives elapsed-time displays and redraws.
    Tick,
    /// Clean shutdown requested by the app itself (quit keystroke, etc.).
    Quit,
}
