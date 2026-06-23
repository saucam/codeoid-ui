//! High-visibility approval banner.
//!
//! A pending tool approval used to surface only as a one-line worker row
//! (same visual weight as the idle/thinking indicator) plus an inline tool
//! card that scrolls away — easy to miss in a busy session. This renders a
//! dedicated, high-contrast banner above the prompt whenever the focused
//! session has a tool awaiting confirmation, so the request is impossible
//! to overlook and the accept/deny keys are spelled out unmissably.

use codeoid_protocol::{SessionMessage, ToolState};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::state::AppState;

/// Height (rows) of the banner zone, including its border. 2 border rows +
/// 2 content rows (the action + the key prompt).
pub const HEIGHT: u16 = 4;

const ACCENT: Color = Color::Yellow;

/// The pending tool approval in a message list, if any: `(tool, description)`.
/// Pure so it can be unit-tested without an [`AppState`]. Scans newest-first
/// so the most recent pending tool wins.
fn pending_tool(msgs: &[SessionMessage]) -> Option<(String, String)> {
    msgs.iter().rev().find_map(|m| {
        let tool = m.tool.as_ref()?;
        match &tool.state {
            ToolState::WaitingConfirmation { description, .. } => {
                Some((tool.name.clone(), description.clone()))
            }
            _ => None,
        }
    })
}

/// The focused session's pending approval, if any.
fn pending(state: &AppState) -> Option<(String, String)> {
    let session = state.sessions.focused()?;
    pending_tool(state.messages.messages(&session.id))
}

/// Whether to reserve a banner row this frame.
#[must_use]
pub fn is_pending(state: &AppState) -> bool {
    pending(state).is_some()
}

pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if let Some((tool, description)) = pending(state) {
        render_banner(frame, area, &tool, &description);
    }
}

/// Draw the banner for a known pending tool. Split out from [`render`] so it
/// can be exercised against a `TestBackend` without standing up an
/// [`AppState`].
fn render_banner(frame: &mut Frame<'_>, area: Rect, tool: &str, description: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
        .title(Span::styled(
            " ⚠ APPROVAL NEEDED ",
            Style::default()
                .bg(ACCENT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Line 1: the action — tool name + what it wants to do.
    let action = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            tool.to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            truncate(description, inner.width.saturating_sub(2) as usize),
            Style::default().fg(Color::White),
        ),
    ]);

    // Line 2: the keys, as high-contrast chips so they read at a glance.
    let chip = |label: &str, bg: Color| {
        Span::styled(
            format!(" {label} "),
            Style::default()
                .bg(bg)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
    };
    let keys = Line::from(vec![
        Span::raw(" "),
        chip("Y", Color::Green),
        Span::styled(
            " approve   ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        chip("D", Color::Red),
        Span::styled(
            " deny   ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        chip("Esc", Color::Gray),
        Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
    ]);

    frame.render_widget(Paragraph::new(vec![action, keys]), inner);
}

/// Truncate to a column budget with an ellipsis. Best-effort by `char`
/// count (descriptions are ASCII-ish); the banner is one line so we never
/// want it to wrap.
fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::{pending_tool, truncate};
    use codeoid_protocol::{
        IdentityType, MessageIdentity, MessageRole, SessionMessage, ToolInfo, ToolState,
    };
    use serde_json::json;

    fn ident() -> MessageIdentity {
        MessageIdentity {
            sub: "spiffe://x/agent/test".into(),
            name: None,
            kind: IdentityType::Agent,
        }
    }

    fn msg(role: MessageRole, tool: Option<ToolInfo>) -> SessionMessage {
        SessionMessage {
            session_id: "s".into(),
            message_id: "m".into(),
            role,
            content: String::new(),
            parts: None,
            identity: ident(),
            tool,
            metadata: None,
            timestamp: "2026-06-23T00:00:00Z".into(),
        }
    }

    fn tool(name: &str, state: ToolState) -> ToolInfo {
        ToolInfo {
            tool_id: "t".into(),
            name: name.into(),
            state,
        }
    }

    #[test]
    fn finds_a_pending_tool() {
        let msgs = vec![
            msg(MessageRole::User, None),
            msg(
                MessageRole::ToolCall,
                Some(tool(
                    "Edit",
                    ToolState::WaitingConfirmation {
                        input: json!({}),
                        description: "edit src/main.rs".into(),
                        approval_id: "a1".into(),
                    },
                )),
            ),
        ];
        assert_eq!(
            pending_tool(&msgs),
            Some(("Edit".into(), "edit src/main.rs".into()))
        );
    }

    #[test]
    fn none_when_no_tool_waiting() {
        let msgs = vec![
            msg(MessageRole::User, None),
            msg(
                MessageRole::ToolCall,
                Some(tool(
                    "Read",
                    ToolState::Completed {
                        success: true,
                        output: None,
                        elapsed_ms: None,
                        confirmed_by: None,
                    },
                )),
            ),
        ];
        assert_eq!(pending_tool(&msgs), None);
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("hi", 5), "hi");
        assert_eq!(truncate("anything", 0), "");
    }

    #[test]
    fn banner_renders_title_action_and_keys() {
        use ratatui::backend::TestBackend;
        use ratatui::buffer::Cell;
        use ratatui::Terminal;

        let mut terminal = Terminal::new(TestBackend::new(60, super::HEIGHT)).unwrap();
        terminal
            .draw(|f| super::render_banner(f, f.area(), "Edit", "edit src/main.rs"))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(Cell::symbol)
            .collect();

        assert!(text.contains("APPROVAL NEEDED"), "title missing in: {text}");
        assert!(text.contains("Edit"), "tool name missing");
        assert!(text.contains("edit src/main.rs"), "description missing");
        assert!(
            text.contains("approve") && text.contains("deny"),
            "key prompts missing"
        );
    }

    #[test]
    fn full_layout_inserts_the_banner_when_a_tool_is_pending() {
        use crate::state::AppState;
        use codeoid_protocol::{AuthOkMsg, SessionInfo, SessionStatus};
        use ratatui::backend::TestBackend;
        use ratatui::buffer::Cell;
        use ratatui::Terminal;

        let mut state = AppState::new(AuthOkMsg {
            identity: MessageIdentity {
                sub: "spiffe://x".into(),
                name: Some("Me".into()),
                kind: IdentityType::Human,
            },
            scopes: vec![],
            protocol_version: Some(1),
        });
        state.sessions.upsert(SessionInfo {
            id: "s1".into(),
            name: "demo".into(),
            workdir: "/tmp".into(),
            status: SessionStatus::WaitingApproval,
            created_by: "u".into(),
            created_at: "2026-06-23T00:00:00Z".into(),
            attached_clients: 0,
            mode: None,
            turns_remaining: None,
            pinned_files: None,
            agent_uri: None,
            subagents: None,
            usage: None,
            rotation: None,
            queued_messages: None,
            model: None,
            fallback_model: None,
        });
        let mut m = msg(
            MessageRole::ToolCall,
            Some(tool(
                "Edit",
                ToolState::WaitingConfirmation {
                    input: json!({}),
                    description: "edit src/main.rs".into(),
                    approval_id: "a1".into(),
                },
            )),
        );
        m.session_id = "s1".into();
        state.messages.apply_message(m);

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| crate::ui::render(f, &mut state)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(Cell::symbol)
            .collect();

        assert!(
            text.contains("APPROVAL NEEDED"),
            "the full layout should insert the approval banner: {text}"
        );
    }
}
