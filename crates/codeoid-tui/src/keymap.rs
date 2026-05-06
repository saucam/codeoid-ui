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
    AutocompleteCommand,
    NextSession,
    PrevSession,
    Interrupt,
    Approve,
    Deny,
    CycleMode,
    ToggleHelp,
    DismissModal,
    PageUp,
    PageDown,
    ScrollUp,
    ScrollDown,
    ScrollToTop,
    ScrollToBottom,
    /// Toggle "verbose" mode for tool output bodies — collapsed (default,
    /// 8-line preview + "+N more" tail) vs full. Bound to `v` in
    /// transcript focus.
    ToggleVerboseToolOutput,
    /// Move the tool-block selection cursor to the next tool_call in the
    /// focused session (cyclic). Bound to `]` in transcript focus.
    SelectNextToolBlock,
    /// Move the tool-block selection cursor to the previous tool_call.
    /// Bound to `[` in transcript focus.
    SelectPrevToolBlock,
    /// Expand or collapse the currently selected tool block (or the most
    /// recent one if no selection). Bound to `Enter` in transcript focus.
    ToggleExpandSelectedToolBlock,
}

pub fn resolve(
    event: KeyEvent,
    prompt_focused: bool,
    modal_open: bool,
    command_mode: bool,
) -> Option<Action> {
    use KeyCode::*;

    // Modal is the highest-priority input mode: Esc / q / ? dismiss, rest
    // is absorbed so keystrokes don't leak into the editor behind it.
    if modal_open {
        return match (event.code, event.modifiers) {
            (Esc, _) | (Char('q'), KeyModifiers::NONE) => Some(Action::DismissModal),
            (Char('?'), _) => Some(Action::ToggleHelp),
            (Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),
            _ => None,
        };
    }

    // Prompt mode — editor gets most keys, but a few chords are always
    // available so the user can interrupt, approve, scroll, and quit
    // without having to blur first.
    if prompt_focused {
        // Tab in command mode = autocomplete. Otherwise it routes to
        // the editor (which inserts an actual tab).
        if command_mode && matches!(event.code, Tab) {
            return Some(Action::AutocompleteCommand);
        }

        return match (event.code, event.modifiers) {
            // Editor-level controls.
            (Esc, _) => Some(Action::BlurPrompt),
            (Enter, KeyModifiers::NONE) => Some(Action::SubmitPrompt),
            (Enter, m) if m.contains(KeyModifiers::SHIFT) => Some(Action::NewlineInPrompt),
            (Char('j'), KeyModifiers::CONTROL) => Some(Action::NewlineInPrompt),

            // Global controls — always reachable while typing.
            (Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),
            (Char('x'), KeyModifiers::CONTROL) => Some(Action::Interrupt),

            // Scroll transcript without blurring.
            (PageUp, _) => Some(Action::PageUp),
            (PageDown, _) => Some(Action::PageDown),
            (Home, m) if m.contains(KeyModifiers::CONTROL) => Some(Action::ScrollToTop),
            (End, m) if m.contains(KeyModifiers::CONTROL) => Some(Action::ScrollToBottom),
            (Up, m) if m.contains(KeyModifiers::CONTROL) => Some(Action::ScrollUp),
            (Down, m) if m.contains(KeyModifiers::CONTROL) => Some(Action::ScrollDown),

            // Session nav (Alt chords — Alt is the standard "meta" key
            // for modal-free actions in TUIs like Helix/Zellij).
            (Char('n'), m) if m.contains(KeyModifiers::ALT) => Some(Action::NextSession),
            (Char('p'), m) if m.contains(KeyModifiers::ALT) => Some(Action::PrevSession),

            // Approvals — Alt+Y / Alt+D so the plain letters stay
            // available for typing.
            (Char('y'), m) if m.contains(KeyModifiers::ALT) => Some(Action::Approve),
            (Char('d'), m) if m.contains(KeyModifiers::ALT) => Some(Action::Deny),

            // Everything else falls through to the TextArea.
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
        (Home, _) | (Char('g'), KeyModifiers::NONE) => Some(Action::ScrollToTop),
        (End, _) | (Char('G'), KeyModifiers::SHIFT) => Some(Action::ScrollToBottom),
        (Up, _) | (Char('k'), KeyModifiers::NONE) => Some(Action::ScrollUp),
        (Down, _) | (Char('j'), KeyModifiers::NONE) => Some(Action::ScrollDown),
        (Char('v'), KeyModifiers::NONE) => Some(Action::ToggleVerboseToolOutput),
        (Char(']'), KeyModifiers::NONE) => Some(Action::SelectNextToolBlock),
        (Char('['), KeyModifiers::NONE) => Some(Action::SelectPrevToolBlock),
        (Enter, KeyModifiers::NONE) => Some(Action::ToggleExpandSelectedToolBlock),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    // ------------ modal open: input is absorbed ------------

    #[test]
    fn modal_absorbs_plain_letters() {
        // Typing `q` with modal open dismisses (doesn't fall through to Quit
        // action, because dismiss comes first). Typing a plain letter
        // doesn't leak into navigation.
        assert_eq!(resolve(key(KeyCode::Char('y')), false, true, false), None);
        assert_eq!(resolve(key(KeyCode::Char('x')), false, true, false), None);
    }

    #[test]
    fn modal_handles_escape_to_dismiss() {
        assert_eq!(
            resolve(key(KeyCode::Esc), false, true, false),
            Some(Action::DismissModal)
        );
    }

    #[test]
    fn modal_treats_q_as_dismiss_not_quit() {
        assert_eq!(
            resolve(key(KeyCode::Char('q')), false, true, false),
            Some(Action::DismissModal)
        );
    }

    #[test]
    fn modal_question_mark_toggles_help() {
        assert_eq!(
            resolve(key(KeyCode::Char('?')), false, true, false),
            Some(Action::ToggleHelp)
        );
    }

    #[test]
    fn modal_ctrl_c_still_quits() {
        assert_eq!(
            resolve(ctrl(KeyCode::Char('c')), false, true, false),
            Some(Action::Quit)
        );
    }

    // ------------ prompt focused: editor routes, modifiers handled ------------

    #[test]
    fn prompt_focused_enter_submits() {
        assert_eq!(
            resolve(key(KeyCode::Enter), true, false, false),
            Some(Action::SubmitPrompt)
        );
    }

    #[test]
    fn prompt_focused_shift_enter_inserts_newline() {
        assert_eq!(
            resolve(shift(KeyCode::Enter), true, false, false),
            Some(Action::NewlineInPrompt)
        );
    }

    #[test]
    fn prompt_focused_ctrl_j_inserts_newline() {
        assert_eq!(
            resolve(ctrl(KeyCode::Char('j')), true, false, false),
            Some(Action::NewlineInPrompt)
        );
    }

    #[test]
    fn prompt_focused_esc_blurs() {
        assert_eq!(
            resolve(key(KeyCode::Esc), true, false, false),
            Some(Action::BlurPrompt)
        );
    }

    #[test]
    fn prompt_focused_plain_typing_passes_through() {
        // Letters that aren't Ctrl+C or Ctrl+J should fall through to None
        // so the app reducer routes them into the TextArea.
        assert_eq!(resolve(key(KeyCode::Char('a')), true, false, false), None);
        assert_eq!(resolve(key(KeyCode::Char('x')), true, false, false), None);
        assert_eq!(resolve(key(KeyCode::Backspace), true, false, false), None);
        assert_eq!(resolve(key(KeyCode::Up), true, false, false), None);
    }

    #[test]
    fn prompt_focused_ctrl_c_still_quits() {
        assert_eq!(
            resolve(ctrl(KeyCode::Char('c')), true, false, false),
            Some(Action::Quit)
        );
    }

    #[test]
    fn prompt_focused_ctrl_x_interrupts() {
        // Users should be able to interrupt mid-type without hitting Esc
        // first; Ctrl+X is the standard chord.
        assert_eq!(
            resolve(ctrl(KeyCode::Char('x')), true, false, false),
            Some(Action::Interrupt)
        );
    }

    #[test]
    fn prompt_focused_pgup_pgdn_scrolls_transcript() {
        assert_eq!(
            resolve(key(KeyCode::PageUp), true, false, false),
            Some(Action::PageUp)
        );
        assert_eq!(
            resolve(key(KeyCode::PageDown), true, false, false),
            Some(Action::PageDown)
        );
    }

    #[test]
    fn prompt_focused_ctrl_updown_scrolls_by_one() {
        assert_eq!(
            resolve(ctrl(KeyCode::Up), true, false, false),
            Some(Action::ScrollUp)
        );
        assert_eq!(
            resolve(ctrl(KeyCode::Down), true, false, false),
            Some(Action::ScrollDown)
        );
    }

    #[test]
    fn prompt_focused_plain_updown_still_routes_to_editor() {
        // Without Ctrl, arrow keys belong to the TextArea (cursor movement).
        assert_eq!(resolve(key(KeyCode::Up), true, false, false), None);
        assert_eq!(resolve(key(KeyCode::Down), true, false, false), None);
    }

    #[test]
    fn prompt_focused_alt_yd_approve_deny() {
        let alt_y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::ALT);
        let alt_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT);
        assert_eq!(resolve(alt_y, true, false, false), Some(Action::Approve));
        assert_eq!(resolve(alt_d, true, false, false), Some(Action::Deny));
    }

    #[test]
    fn prompt_focused_alt_np_switches_sessions() {
        let alt_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT);
        let alt_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT);
        assert_eq!(resolve(alt_n, true, false, false), Some(Action::NextSession));
        assert_eq!(resolve(alt_p, true, false, false), Some(Action::PrevSession));
    }

    #[test]
    fn prompt_focused_nav_keys_dont_leak() {
        // `n` / `p` without Alt are real letters — must NOT steal focus.
        assert_eq!(resolve(key(KeyCode::Char('n')), true, false, false), None);
        assert_eq!(resolve(key(KeyCode::Char('p')), true, false, false), None);
        assert_eq!(resolve(key(KeyCode::Char('q')), true, false, false), None);
        assert_eq!(resolve(key(KeyCode::Char('y')), true, false, false), None);
        assert_eq!(resolve(key(KeyCode::Char('d')), true, false, false), None);
    }

    #[test]
    fn command_mode_tab_autocompletes() {
        // With command_mode=true, Tab becomes the autocomplete trigger.
        assert_eq!(
            resolve(key(KeyCode::Tab), true, false, true),
            Some(Action::AutocompleteCommand)
        );
    }

    #[test]
    fn command_mode_off_tab_still_reaches_editor() {
        // Without command mode, Tab falls through so the TextArea inserts
        // a real tab character (users rarely type tabs in prompts, but
        // the behaviour should be predictable).
        assert_eq!(resolve(key(KeyCode::Tab), true, false, false), None);
    }

    // ------------ nav mode (no prompt focus) ------------

    #[test]
    fn nav_letter_q_quits() {
        assert_eq!(
            resolve(key(KeyCode::Char('q')), false, false, false),
            Some(Action::Quit)
        );
    }

    #[test]
    fn nav_tab_focuses_prompt() {
        assert_eq!(
            resolve(key(KeyCode::Tab), false, false, false),
            Some(Action::FocusPrompt)
        );
    }

    #[test]
    fn nav_i_focuses_prompt() {
        assert_eq!(
            resolve(key(KeyCode::Char('i')), false, false, false),
            Some(Action::FocusPrompt)
        );
    }

    #[test]
    fn nav_next_prev_session() {
        assert_eq!(
            resolve(key(KeyCode::Right), false, false, false),
            Some(Action::NextSession)
        );
        assert_eq!(
            resolve(key(KeyCode::Left), false, false, false),
            Some(Action::PrevSession)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('n')), false, false, false),
            Some(Action::NextSession)
        );
    }

    #[test]
    fn nav_approve_deny() {
        assert_eq!(
            resolve(key(KeyCode::Char('y')), false, false, false),
            Some(Action::Approve)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('d')), false, false, false),
            Some(Action::Deny)
        );
    }

    #[test]
    fn nav_interrupt() {
        assert_eq!(
            resolve(ctrl(KeyCode::Char('x')), false, false, false),
            Some(Action::Interrupt)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('.')), false, false, false),
            Some(Action::Interrupt)
        );
    }

    #[test]
    fn nav_scroll() {
        assert_eq!(
            resolve(key(KeyCode::PageUp), false, false, false),
            Some(Action::PageUp)
        );
        assert_eq!(
            resolve(key(KeyCode::PageDown), false, false, false),
            Some(Action::PageDown)
        );
        assert_eq!(
            resolve(key(KeyCode::Up), false, false, false),
            Some(Action::ScrollUp)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('k')), false, false, false),
            Some(Action::ScrollUp)
        );
    }
}
