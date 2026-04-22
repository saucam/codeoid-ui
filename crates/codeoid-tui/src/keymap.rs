//! Keybinding table. Keep every shortcut in one place so the help modal
//! can render it and tests can assert it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// High-level actions the app reducer understands. Keystrokes resolve to
/// one of these; the reducer never pattern-matches on `KeyCode` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    FocusPrompt,
    BlurPrompt,
    SubmitPrompt,
    NewlineInPrompt,
    NextSession,
    PrevSession,
    Interrupt,
    Approve,
    Deny,
    CycleMode,
    ToggleHelp,
    PageUp,
    PageDown,
    ScrollUp,
    ScrollDown,
}

pub fn resolve(event: KeyEvent, prompt_focused: bool) -> Option<Action> {
    use KeyCode::*;

    // When typing in the prompt, only a small set of chords can steal focus.
    if prompt_focused {
        return match (event.code, event.modifiers) {
            (Esc, _) => Some(Action::BlurPrompt),
            (Enter, KeyModifiers::NONE) => Some(Action::SubmitPrompt),
            (Enter, m) if m.contains(KeyModifiers::SHIFT) => Some(Action::NewlineInPrompt),
            (Char('j'), KeyModifiers::CONTROL) => Some(Action::NewlineInPrompt),
            (Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),
            _ => None,
        };
    }

    match (event.code, event.modifiers) {
        (Char('q'), KeyModifiers::NONE) | (Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),
        (Char('i'), KeyModifiers::NONE) | (Tab, _) => Some(Action::FocusPrompt),
        (Char('?'), _) => Some(Action::ToggleHelp),
        (Char('n'), KeyModifiers::NONE) | (Right, _) => Some(Action::NextSession),
        (Char('p'), KeyModifiers::NONE) | (Left, _) => Some(Action::PrevSession),
        (Char('x'), KeyModifiers::CONTROL) | (Char('.'), _) => Some(Action::Interrupt),
        (Char('y'), _) => Some(Action::Approve),
        (Char('d'), _) => Some(Action::Deny),
        (Char('m'), _) => Some(Action::CycleMode),
        (PageUp, _) => Some(Action::PageUp),
        (PageDown, _) => Some(Action::PageDown),
        (Up, _) | (Char('k'), KeyModifiers::NONE) => Some(Action::ScrollUp),
        (Down, _) | (Char('j'), KeyModifiers::NONE) => Some(Action::ScrollDown),
        _ => None,
    }
}
