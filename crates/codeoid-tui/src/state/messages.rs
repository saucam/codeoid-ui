//! Per-session message buffer + delta reducer.
//!
//! A `MessageStore` owns one `Vec<SessionMessage>` per session keyed by
//! session id. It exposes pure mutations — `apply_message`, `apply_delta`,
//! `replace_scrollback` — so the app reducer stays test-friendly and free
//! of Tokio or Ratatui.

use std::collections::HashMap;

use codeoid_protocol::{ContentPart, SessionMessage, SessionMessageDelta};
use tracing::debug;

/// How to merge an incoming `SessionMessage`:
/// * If a message with the same `message_id` already exists in the buffer,
///   replace it (daemon re-broadcast or scrollback hit).
/// * Otherwise append.
fn upsert(buf: &mut Vec<SessionMessage>, m: SessionMessage) {
    if let Some(slot) = buf.iter_mut().find(|sm| sm.message_id == m.message_id) {
        *slot = m;
    } else {
        buf.push(m);
    }
}

#[derive(Debug, Default)]
pub struct MessageStore {
    by_session: HashMap<String, Vec<SessionMessage>>,
}

impl MessageStore {
    pub fn messages(&self, session_id: &str) -> &[SessionMessage] {
        self.by_session
            .get(session_id)
            .map_or::<&[SessionMessage], _>(&[], Vec::as_slice)
    }

    pub fn apply_message(&mut self, msg: SessionMessage) {
        let buf = self.by_session.entry(msg.session_id.clone()).or_default();
        upsert(buf, msg);
    }

    /// Apply a streaming delta. Drops with a `debug!` trace when we don't
    /// have the parent message — the protocol guarantees scrollback
    /// replay will catch us up on re-attach.
    pub fn apply_delta(&mut self, delta: SessionMessageDelta) {
        let Some(buf) = self.by_session.get_mut(&delta.session_id) else {
            debug!(
                session_id = %delta.session_id,
                message_id = %delta.message_id,
                "delta for unknown session — dropped (scrollback will recover)"
            );
            return;
        };
        let Some(target) = buf.iter_mut().find(|m| m.message_id == delta.message_id) else {
            debug!(
                session_id = %delta.session_id,
                message_id = %delta.message_id,
                "delta for unknown message — dropped (evicted or pre-attach)"
            );
            return;
        };

        if let Some(s) = delta.content_append {
            target.content.push_str(&s);
        }

        if let Some(parts_append) = delta.parts_append {
            target
                .parts
                .get_or_insert_with(Vec::new)
                .extend(parts_append);
        }

        if let Some(updates) = delta.parts_update {
            if let Some(parts) = target.parts.as_mut() {
                for upd in updates {
                    let idx = upd.index as usize;
                    if idx < parts.len() {
                        parts[idx] = upd.part;
                    } else {
                        parts.push(upd.part);
                    }
                }
            }
        }

        // Tool state updates are WHOLESALE replacements, not field patches.
        // See Codeoid CLAUDE.md / protocol/types.ts SessionMessageDelta docs.
        if let Some(new_state) = delta.tool_state_update {
            if let Some(tool) = target.tool.as_mut() {
                debug!(
                    message_id = %delta.message_id,
                    tool_id = %tool.tool_id,
                    tool_name = %tool.name,
                    from = ?tool.state.phase(),
                    to = ?new_state.phase(),
                    "tool state transition"
                );
                tool.state = new_state;
            } else {
                debug!(
                    message_id = %delta.message_id,
                    "tool_state_update for message without a tool — ignored"
                );
            }
        }

        target.timestamp = delta.timestamp;
    }

    pub fn replace_scrollback(&mut self, session_id: String, messages: Vec<SessionMessage>) {
        self.by_session.insert(session_id, messages);
    }

    /// Strip the cheapest possible text preview from a message, for list
    /// views that don't want the full rich part tree.
    pub fn preview(msg: &SessionMessage) -> String {
        if !msg.content.is_empty() {
            return msg.content.clone();
        }
        if let Some(parts) = &msg.parts {
            for p in parts {
                if let ContentPart::Text { text, .. } = p {
                    return text.clone();
                }
            }
        }
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeoid_protocol::{
        CancelReason, ContentPart, IdentityType, MessageIdentity, MessageRole, ToolInfo,
        ToolState,
    };

    fn mk_msg(session_id: &str, message_id: &str) -> SessionMessage {
        SessionMessage {
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            role: MessageRole::Assistant,
            content: String::new(),
            parts: None,
            identity: MessageIdentity {
                sub: "s".into(),
                name: None,
                kind: IdentityType::Agent,
            },
            tool: None,
            metadata: None,
            timestamp: "2026-04-22T00:00:00Z".into(),
        }
    }

    fn mk_delta(session_id: &str, message_id: &str) -> SessionMessageDelta {
        SessionMessageDelta {
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            content_append: None,
            parts_append: None,
            parts_update: None,
            tool_state_update: None,
            timestamp: "2026-04-22T00:00:01Z".into(),
        }
    }

    #[test]
    fn apply_message_appends_when_new() {
        let mut store = MessageStore::default();
        store.apply_message(mk_msg("s1", "m1"));
        store.apply_message(mk_msg("s1", "m2"));
        let msgs = store.messages("s1");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].message_id, "m1");
        assert_eq!(msgs[1].message_id, "m2");
    }

    #[test]
    fn apply_message_replaces_when_message_id_matches() {
        let mut store = MessageStore::default();
        let mut first = mk_msg("s1", "m1");
        first.content = "old".into();
        store.apply_message(first);

        let mut second = mk_msg("s1", "m1");
        second.content = "new".into();
        store.apply_message(second);

        let msgs = store.messages("s1");
        assert_eq!(msgs.len(), 1, "same message_id should upsert, not append");
        assert_eq!(msgs[0].content, "new");
    }

    #[test]
    fn messages_returns_empty_slice_for_unknown_session() {
        let store = MessageStore::default();
        assert!(store.messages("nope").is_empty());
    }

    #[test]
    fn content_append_concatenates_deltas() {
        let mut store = MessageStore::default();
        store.apply_message(mk_msg("s1", "m1"));
        let mut d = mk_delta("s1", "m1");
        d.content_append = Some("hello ".into());
        store.apply_delta(d);

        let mut d2 = mk_delta("s1", "m1");
        d2.content_append = Some("world".into());
        store.apply_delta(d2);

        assert_eq!(store.messages("s1")[0].content, "hello world");
    }

    #[test]
    fn parts_append_grows_parts_vec() {
        let mut store = MessageStore::default();
        store.apply_message(mk_msg("s1", "m1"));
        let mut d = mk_delta("s1", "m1");
        d.parts_append = Some(vec![ContentPart::Text {
            text: "first".into(),
            markdown: None,
        }]);
        store.apply_delta(d);
        let mut d2 = mk_delta("s1", "m1");
        d2.parts_append = Some(vec![ContentPart::Text {
            text: "second".into(),
            markdown: None,
        }]);
        store.apply_delta(d2);

        let parts = store.messages("s1")[0].parts.as_ref().unwrap();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn tool_state_update_replaces_wholesale_not_merged() {
        let mut store = MessageStore::default();
        let mut msg = mk_msg("s1", "m1");
        msg.tool = Some(ToolInfo {
            tool_id: "t".into(),
            name: "Bash".into(),
            state: ToolState::Executing {
                progress: Some("running".into()),
                elapsed_ms: Some(100),
            },
        });
        store.apply_message(msg);

        let mut d = mk_delta("s1", "m1");
        d.tool_state_update = Some(ToolState::Completed {
            success: true,
            output: Some("done".into()),
            elapsed_ms: Some(250),
            confirmed_by: None,
        });
        store.apply_delta(d);

        let tool = store.messages("s1")[0].tool.as_ref().unwrap();
        match &tool.state {
            ToolState::Completed {
                success,
                output,
                elapsed_ms,
                ..
            } => {
                assert!(*success);
                assert_eq!(output.as_deref(), Some("done"));
                assert_eq!(*elapsed_ms, Some(250));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn delta_without_parent_is_ignored() {
        let mut store = MessageStore::default();
        let mut d = mk_delta("s1", "missing");
        d.content_append = Some("x".into());
        store.apply_delta(d);
        assert!(store.messages("s1").is_empty());
    }

    #[test]
    fn delta_for_unknown_session_is_ignored() {
        let mut store = MessageStore::default();
        store.apply_message(mk_msg("other", "m1"));
        let mut d = mk_delta("unknown", "m1");
        d.content_append = Some("x".into());
        store.apply_delta(d);
        // "other" session's content must not be touched.
        assert_eq!(store.messages("other")[0].content, "");
    }

    #[test]
    fn delta_tool_state_requires_existing_tool() {
        let mut store = MessageStore::default();
        // Message has no tool — delta carries tool_state_update → ignored
        store.apply_message(mk_msg("s1", "m1"));
        let mut d = mk_delta("s1", "m1");
        d.tool_state_update = Some(ToolState::Cancelled {
            reason: CancelReason::Denied,
            message: None,
        });
        store.apply_delta(d);

        assert!(store.messages("s1")[0].tool.is_none());
    }

    #[test]
    fn replace_scrollback_overwrites() {
        let mut store = MessageStore::default();
        store.apply_message(mk_msg("s1", "a"));
        store.apply_message(mk_msg("s1", "b"));
        store.replace_scrollback(
            "s1".into(),
            vec![mk_msg("s1", "x"), mk_msg("s1", "y"), mk_msg("s1", "z")],
        );
        let msgs = store.messages("s1");
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].message_id, "x");
        assert_eq!(msgs[2].message_id, "z");
    }

    #[test]
    fn replace_scrollback_does_not_affect_other_sessions() {
        let mut store = MessageStore::default();
        store.apply_message(mk_msg("s1", "m1"));
        store.apply_message(mk_msg("s2", "m1"));
        store.replace_scrollback("s1".into(), vec![]);
        assert!(store.messages("s1").is_empty());
        assert_eq!(store.messages("s2").len(), 1);
    }

    #[test]
    fn parts_update_replaces_at_index() {
        let mut store = MessageStore::default();
        let mut m = mk_msg("s1", "m1");
        m.parts = Some(vec![
            ContentPart::Text {
                text: "a".into(),
                markdown: None,
            },
            ContentPart::Text {
                text: "b".into(),
                markdown: None,
            },
        ]);
        store.apply_message(m);

        let mut d = mk_delta("s1", "m1");
        d.parts_update = Some(vec![codeoid_protocol::message::PartsUpdate {
            index: 1,
            part: ContentPart::Text {
                text: "B-REPLACED".into(),
                markdown: None,
            },
        }]);
        store.apply_delta(d);

        let parts = store.messages("s1")[0].parts.as_ref().unwrap();
        if let ContentPart::Text { text, .. } = &parts[1] {
            assert_eq!(text, "B-REPLACED");
        } else {
            panic!("expected text part");
        }
    }

    #[test]
    fn preview_prefers_content_string() {
        let mut m = mk_msg("s1", "m1");
        m.content = "plain".into();
        m.parts = Some(vec![ContentPart::Text {
            text: "rich".into(),
            markdown: None,
        }]);
        assert_eq!(MessageStore::preview(&m), "plain");
    }

    #[test]
    fn preview_falls_back_to_first_text_part() {
        let mut m = mk_msg("s1", "m1");
        m.content = String::new();
        m.parts = Some(vec![
            ContentPart::Code {
                code: "ignored".into(),
                language: None,
                file_path: None,
            },
            ContentPart::Text {
                text: "shown".into(),
                markdown: None,
            },
        ]);
        assert_eq!(MessageStore::preview(&m), "shown");
    }
}
