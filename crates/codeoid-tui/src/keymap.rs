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
    /// AskUserQuestion modal: toggle option N (1-based) for the focused
    /// question. Number keys 1-4 in the AskUserQuestion modal. For
    /// single-select questions, replaces; for multi-select, toggles.
    AskToggleOption(u8),
    /// AskUserQuestion modal: cycle to the next question (wraps).
    AskNextQuestion,
    /// AskUserQuestion modal: cycle to the previous question (wraps).
    AskPrevQuestion,
    /// AskUserQuestion modal: submit answers — only effective when every
    /// question has at least one selection. Sends `session.approve`
    /// with the `answers` map as `updatedInput`.
    AskSubmit,
    /// AskUserQuestion modal: cancel and send a denial back to Claude.
    AskCancel,
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

/// What kind of modal is currently open. The AskUserQuestion form needs
/// its own keymap (number keys, Tab, Enter to submit) that differs from
/// the generic absorb-everything-else behaviour for help / confirm /
/// capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKind {
    None,
    Generic,
    AskUserQuestion,
}

pub fn resolve(
    event: KeyEvent,
    prompt_focused: bool,
    modal_kind: ModalKind,
    command_mode: bool,
) -> Option<Action> {
    use KeyCode::*;

    // AskUserQuestion modal: number keys toggle options, Tab navigates
    // between questions, Enter submits, Esc cancels (and sends a deny
    // back to Claude rather than just dismissing the modal).
    if matches!(modal_kind, ModalKind::AskUserQuestion) {
        return match (event.code, event.modifiers) {
            (Esc, _) => Some(Action::AskCancel),
            (Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),
            (Char('?'), _) => Some(Action::ToggleHelp),
            (Tab, _) | (Char('n'), _) | (Down, _) | (Char('j'), _) => Some(Action::AskNextQuestion),
            (BackTab, _) | (Char('p'), _) | (Up, _) | (Char('k'), _) => {
                Some(Action::AskPrevQuestion)
            }
            (Enter, _) => Some(Action::AskSubmit),
            (Char(c @ '1'..='9'), KeyModifiers::NONE) => {
                Some(Action::AskToggleOption((c as u8) - b'0'))
            }
            _ => None,
        };
    }

    // Generic modals (Help / ConfirmDestroy / Capabilities) — Esc / q
    // dismiss, the rest is absorbed so keystrokes don't leak through.
    if matches!(modal_kind, ModalKind::Generic) {
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
        assert_eq!(
            resolve(key(KeyCode::Char('y')), false, ModalKind::Generic, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('x')), false, ModalKind::Generic, false),
            None
        );
    }

    #[test]
    fn modal_handles_escape_to_dismiss() {
        assert_eq!(
            resolve(key(KeyCode::Esc), false, ModalKind::Generic, false),
            Some(Action::DismissModal)
        );
    }

    #[test]
    fn modal_treats_q_as_dismiss_not_quit() {
        assert_eq!(
            resolve(key(KeyCode::Char('q')), false, ModalKind::Generic, false),
            Some(Action::DismissModal)
        );
    }

    #[test]
    fn modal_question_mark_toggles_help() {
        assert_eq!(
            resolve(key(KeyCode::Char('?')), false, ModalKind::Generic, false),
            Some(Action::ToggleHelp)
        );
    }

    #[test]
    fn modal_ctrl_c_still_quits() {
        assert_eq!(
            resolve(ctrl(KeyCode::Char('c')), false, ModalKind::Generic, false),
            Some(Action::Quit)
        );
    }

    // ------------ prompt focused: editor routes, modifiers handled ------------

    #[test]
    fn prompt_focused_enter_submits() {
        assert_eq!(
            resolve(key(KeyCode::Enter), true, ModalKind::None, false),
            Some(Action::SubmitPrompt)
        );
    }

    #[test]
    fn prompt_focused_shift_enter_inserts_newline() {
        assert_eq!(
            resolve(shift(KeyCode::Enter), true, ModalKind::None, false),
            Some(Action::NewlineInPrompt)
        );
    }

    #[test]
    fn prompt_focused_ctrl_j_inserts_newline() {
        assert_eq!(
            resolve(ctrl(KeyCode::Char('j')), true, ModalKind::None, false),
            Some(Action::NewlineInPrompt)
        );
    }

    #[test]
    fn prompt_focused_esc_blurs() {
        assert_eq!(
            resolve(key(KeyCode::Esc), true, ModalKind::None, false),
            Some(Action::BlurPrompt)
        );
    }

    #[test]
    fn prompt_focused_plain_typing_passes_through() {
        // Letters that aren't Ctrl+C or Ctrl+J should fall through to None
        // so the app reducer routes them into the TextArea.
        assert_eq!(
            resolve(key(KeyCode::Char('a')), true, ModalKind::None, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('x')), true, ModalKind::None, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Backspace), true, ModalKind::None, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Up), true, ModalKind::None, false),
            None
        );
    }

    #[test]
    fn prompt_focused_ctrl_c_still_quits() {
        assert_eq!(
            resolve(ctrl(KeyCode::Char('c')), true, ModalKind::None, false),
            Some(Action::Quit)
        );
    }

    #[test]
    fn prompt_focused_ctrl_x_interrupts() {
        // Users should be able to interrupt mid-type without hitting Esc
        // first; Ctrl+X is the standard chord.
        assert_eq!(
            resolve(ctrl(KeyCode::Char('x')), true, ModalKind::None, false),
            Some(Action::Interrupt)
        );
    }

    #[test]
    fn prompt_focused_pgup_pgdn_scrolls_transcript() {
        assert_eq!(
            resolve(key(KeyCode::PageUp), true, ModalKind::None, false),
            Some(Action::PageUp)
        );
        assert_eq!(
            resolve(key(KeyCode::PageDown), true, ModalKind::None, false),
            Some(Action::PageDown)
        );
    }

    #[test]
    fn prompt_focused_ctrl_updown_scrolls_by_one() {
        assert_eq!(
            resolve(ctrl(KeyCode::Up), true, ModalKind::None, false),
            Some(Action::ScrollUp)
        );
        assert_eq!(
            resolve(ctrl(KeyCode::Down), true, ModalKind::None, false),
            Some(Action::ScrollDown)
        );
    }

    #[test]
    fn prompt_focused_plain_updown_still_routes_to_editor() {
        // Without Ctrl, arrow keys belong to the TextArea (cursor movement).
        assert_eq!(
            resolve(key(KeyCode::Up), true, ModalKind::None, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Down), true, ModalKind::None, false),
            None
        );
    }

    #[test]
    fn prompt_focused_alt_yd_approve_deny() {
        let alt_y = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::ALT);
        let alt_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT);
        assert_eq!(
            resolve(alt_y, true, ModalKind::None, false),
            Some(Action::Approve)
        );
        assert_eq!(
            resolve(alt_d, true, ModalKind::None, false),
            Some(Action::Deny)
        );
    }

    #[test]
    fn prompt_focused_alt_np_switches_sessions() {
        let alt_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT);
        let alt_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT);
        assert_eq!(
            resolve(alt_n, true, ModalKind::None, false),
            Some(Action::NextSession)
        );
        assert_eq!(
            resolve(alt_p, true, ModalKind::None, false),
            Some(Action::PrevSession)
        );
    }

    #[test]
    fn prompt_focused_nav_keys_dont_leak() {
        // `n` / `p` without Alt are real letters — must NOT steal focus.
        assert_eq!(
            resolve(key(KeyCode::Char('n')), true, ModalKind::None, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('p')), true, ModalKind::None, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('q')), true, ModalKind::None, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('y')), true, ModalKind::None, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('d')), true, ModalKind::None, false),
            None
        );
    }

    #[test]
    fn command_mode_tab_autocompletes() {
        // With command_mode=true, Tab becomes the autocomplete trigger.
        assert_eq!(
            resolve(key(KeyCode::Tab), true, ModalKind::None, true),
            Some(Action::AutocompleteCommand)
        );
    }

    #[test]
    fn command_mode_off_tab_still_reaches_editor() {
        // Without command mode, Tab falls through so the TextArea inserts
        // a real tab character (users rarely type tabs in prompts, but
        // the behaviour should be predictable).
        assert_eq!(
            resolve(key(KeyCode::Tab), true, ModalKind::None, false),
            None
        );
    }

    // ------------ nav mode (no prompt focus) ------------

    #[test]
    fn nav_letter_q_quits() {
        assert_eq!(
            resolve(key(KeyCode::Char('q')), false, ModalKind::None, false),
            Some(Action::Quit)
        );
    }

    #[test]
    fn nav_tab_focuses_prompt() {
        assert_eq!(
            resolve(key(KeyCode::Tab), false, ModalKind::None, false),
            Some(Action::FocusPrompt)
        );
    }

    #[test]
    fn nav_i_focuses_prompt() {
        assert_eq!(
            resolve(key(KeyCode::Char('i')), false, ModalKind::None, false),
            Some(Action::FocusPrompt)
        );
    }

    #[test]
    fn nav_next_prev_session() {
        assert_eq!(
            resolve(key(KeyCode::Right), false, ModalKind::None, false),
            Some(Action::NextSession)
        );
        assert_eq!(
            resolve(key(KeyCode::Left), false, ModalKind::None, false),
            Some(Action::PrevSession)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('n')), false, ModalKind::None, false),
            Some(Action::NextSession)
        );
    }

    #[test]
    fn nav_approve_deny() {
        assert_eq!(
            resolve(key(KeyCode::Char('y')), false, ModalKind::None, false),
            Some(Action::Approve)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('d')), false, ModalKind::None, false),
            Some(Action::Deny)
        );
    }

    #[test]
    fn nav_interrupt() {
        assert_eq!(
            resolve(ctrl(KeyCode::Char('x')), false, ModalKind::None, false),
            Some(Action::Interrupt)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('.')), false, ModalKind::None, false),
            Some(Action::Interrupt)
        );
    }

    #[test]
    fn nav_scroll() {
        assert_eq!(
            resolve(key(KeyCode::PageUp), false, ModalKind::None, false),
            Some(Action::PageUp)
        );
        assert_eq!(
            resolve(key(KeyCode::PageDown), false, ModalKind::None, false),
            Some(Action::PageDown)
        );
        assert_eq!(
            resolve(key(KeyCode::Up), false, ModalKind::None, false),
            Some(Action::ScrollUp)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('k')), false, ModalKind::None, false),
            Some(Action::ScrollUp)
        );
    }
}
