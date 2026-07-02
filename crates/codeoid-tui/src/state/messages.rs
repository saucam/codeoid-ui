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
///
/// Searches back-to-front: re-broadcasts and streaming updates target
/// the NEWEST message, so a front-to-back scan is the worst case on
/// long transcripts (O(N) per event, felt as prompt lag after many
/// turns). Should duplicate ids ever appear (daemon bug), the newest
/// occurrence wins — consistent with [`MessageStore::apply_delta`].
fn upsert(buf: &mut Vec<SessionMessage>, m: SessionMessage) {
    if let Some(slot) = buf
        .iter_mut()
        .rev()
        .find(|sm| sm.message_id == m.message_id)
    {
        *slot = m;
    } else {
        buf.push(m);
    }
}

#[derive(Debug, Default)]
pub struct MessageStore {
    by_session: HashMap<String, Vec<SessionMessage>>,
    /// Per-message version counter, bumped on every mutation. Used as
    /// the cache key in [`crate::state::RenderCache`] so cached styled
    /// lines stay valid until the underlying message changes.
    versions: HashMap<String, u64>,
    /// Per-session epoch, bumped on any mutation that affects the
    /// session's transcript. Lets the scrollback assembled-lines cache
    /// skip work in O(1) when nothing has changed since last frame.
    session_epoch: HashMap<String, u64>,
}

impl MessageStore {
    pub fn messages(&self, session_id: &str) -> &[SessionMessage] {
        self.by_session
            .get(session_id)
            .map_or::<&[SessionMessage], _>(&[], Vec::as_slice)
    }

    /// Monotonic version number for a message. Returns 0 for unknown
    /// ids; any version >= 1 means "we've seen this id at least once."
    /// Wrapping arithmetic on u64 means overflow is theoretical, not
    /// practical.
    #[must_use]
    pub fn version_of(&self, message_id: &str) -> u64 {
        self.versions.get(message_id).copied().unwrap_or(0)
    }

    /// Monotonic epoch for a whole session. Bumped on any mutation to
    /// the session's transcript. Returns 0 for sessions we've never
    /// seen, which trivially mismatches any cached non-zero epoch.
    #[must_use]
    pub fn epoch_of_session(&self, session_id: &str) -> u64 {
        self.session_epoch.get(session_id).copied().unwrap_or(0)
    }

    fn bump(&mut self, message_id: &str) {
        let entry = self.versions.entry(message_id.to_string()).or_insert(0);
        *entry = entry.wrapping_add(1);
    }

    fn bump_session(&mut self, session_id: &str) {
        let entry = self
            .session_epoch
            .entry(session_id.to_string())
            .or_insert(0);
        *entry = entry.wrapping_add(1);
    }

    pub fn apply_message(&mut self, msg: SessionMessage) {
        let mid = msg.message_id.clone();
        let sid = msg.session_id.clone();
        let buf = self.by_session.entry(msg.session_id.clone()).or_default();
        upsert(buf, msg);
        self.bump(&mid);
        self.bump_session(&sid);
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
        // Deltas stream into the newest message — search from the back
        // so the common case is O(1) instead of a scan over the whole
        // transcript on every streamed chunk.
        let Some(target) = buf
            .iter_mut()
            .rev()
            .find(|m| m.message_id == delta.message_id)
        else {
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

        let mid = delta.message_id.clone();
        let sid = delta.session_id.clone();
        self.bump(&mid);
        self.bump_session(&sid);
    }

    pub fn replace_scrollback(&mut self, session_id: String, messages: Vec<SessionMessage>) {
        // Bump every message's version so any prior cached render is
        // invalidated. `replace_scrollback` runs on attach / re-attach,
        // when content may have changed underneath us.
        for m in &messages {
            self.bump(&m.message_id);
        }
        self.bump_session(&session_id);
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
        CancelReason, ContentPart, IdentityType, MessageIdentity, MessageRole, ToolInfo, ToolState,
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
    fn apply_delta_targets_newest_when_duplicate_ids_exist() {
        // Duplicate message ids can't be produced through upsert, but a
        // buggy daemon replay could inject them via replace_scrollback.
        // The back-to-front lookup must patch the NEWEST occurrence and
        // leave the older duplicate untouched.
        let mut store = MessageStore::default();
        let mut old_dup = mk_msg("s1", "dup");
        old_dup.content = "old".into();
        let mut new_dup = mk_msg("s1", "dup");
        new_dup.content = "new".into();
        store.replace_scrollback("s1".into(), vec![old_dup, mk_msg("s1", "mid"), new_dup]);

        let mut d = mk_delta("s1", "dup");
        d.content_append = Some("+delta".into());
        store.apply_delta(d);

        let msgs = store.messages("s1");
        assert_eq!(msgs[0].content, "old", "older duplicate must be untouched");
        assert_eq!(
            msgs[2].content, "new+delta",
            "newest duplicate gets the delta"
        );
    }

    #[test]
    fn apply_delta_still_reaches_older_messages() {
        // The reverse scan is an optimization, not a truncation — deltas
        // for non-tail messages must still land.
        let mut store = MessageStore::default();
        store.apply_message(mk_msg("s1", "m1"));
        store.apply_message(mk_msg("s1", "m2"));
        store.apply_message(mk_msg("s1", "m3"));

        let mut d = mk_delta("s1", "m1");
        d.content_append = Some("first".into());
        store.apply_delta(d);

        assert_eq!(store.messages("s1")[0].content, "first");
    }

    #[test]
    fn upsert_replaces_newest_duplicate() {
        let mut store = MessageStore::default();
        let mut old_dup = mk_msg("s1", "dup");
        old_dup.content = "old".into();
        let mut new_dup = mk_msg("s1", "dup");
        new_dup.content = "new".into();
        store.replace_scrollback("s1".into(), vec![old_dup, new_dup]);

        let mut incoming = mk_msg("s1", "dup");
        incoming.content = "replaced".into();
        store.apply_message(incoming);

        let msgs = store.messages("s1");
        assert_eq!(msgs.len(), 2, "upsert must not append a third copy");
        assert_eq!(msgs[0].content, "old");
        assert_eq!(msgs[1].content, "replaced");
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
