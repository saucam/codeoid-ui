//! Per-session message buffer + delta reducer.
//!
//! A `MessageStore` owns one `Vec<SessionMessage>` per session keyed by
//! session id. It exposes pure-ish mutations — `apply_message`,
//! `apply_delta`, `replace_scrollback` — so the app reducer can stay test-
//! friendly.

use std::collections::HashMap;

use codeoid_protocol::{ContentPart, SessionMessage, SessionMessageDelta};

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

    pub fn apply_delta(&mut self, delta: SessionMessageDelta) {
        let Some(buf) = self.by_session.get_mut(&delta.session_id) else {
            // We don't have the parent message yet — scrollback replay will
            // deliver the merged state on reconnect, so dropping this delta
            // is safe. (Protocol docs: "clients can ignore deltas and rely
            // on the scrollback replay to get the complete state".)
            return;
        };
        let Some(target) = buf.iter_mut().find(|m| m.message_id == delta.message_id) else {
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
                tool.state = new_state;
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
        IdentityType, MessageIdentity, MessageRole, ToolInfo, ToolState,
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

    #[test]
    fn content_append_concatenates() {
        let mut store = MessageStore::default();
        store.apply_message(mk_msg("s1", "m1"));
        store.apply_delta(SessionMessageDelta {
            session_id: "s1".into(),
            message_id: "m1".into(),
            content_append: Some("hello ".into()),
            parts_append: None,
            parts_update: None,
            tool_state_update: None,
            timestamp: "2026-04-22T00:00:01Z".into(),
        });
        store.apply_delta(SessionMessageDelta {
            session_id: "s1".into(),
            message_id: "m1".into(),
            content_append: Some("world".into()),
            parts_append: None,
            parts_update: None,
            tool_state_update: None,
            timestamp: "2026-04-22T00:00:02Z".into(),
        });
        assert_eq!(store.messages("s1")[0].content, "hello world");
    }

    #[test]
    fn tool_state_update_replaces_wholesale() {
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

        // Completed state carries `success` + `output`. A field-by-field
        // merge would leave stale `progress` around; wholesale replace drops it.
        store.apply_delta(SessionMessageDelta {
            session_id: "s1".into(),
            message_id: "m1".into(),
            content_append: None,
            parts_append: None,
            parts_update: None,
            tool_state_update: Some(ToolState::Completed {
                success: true,
                output: Some("done".into()),
                elapsed_ms: Some(250),
                confirmed_by: None,
            }),
            timestamp: "2026-04-22T00:00:03Z".into(),
        });

        let stored = &store.messages("s1")[0];
        let tool = stored.tool.as_ref().expect("tool present");
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
        // No parent — delta should no-op rather than panic.
        store.apply_delta(SessionMessageDelta {
            session_id: "s1".into(),
            message_id: "missing".into(),
            content_append: Some("x".into()),
            parts_append: None,
            parts_update: None,
            tool_state_update: None,
            timestamp: "2026-04-22T00:00:00Z".into(),
        });
        assert!(store.messages("s1").is_empty());
    }
}
