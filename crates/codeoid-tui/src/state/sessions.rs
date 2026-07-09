//! Session list + focus state.

use std::collections::HashMap;

use codeoid_protocol::SessionInfo;

#[derive(Debug, Default)]
pub struct SessionList {
    items: Vec<SessionInfo>,
    focused: Option<usize>,
}

impl SessionList {
    pub fn replace(&mut self, items: Vec<SessionInfo>) {
        let previously_focused = self.focused_id().map(ToString::to_string);
        // Session ids are daemon-minted UUIDs and unique by contract,
        // but a Vec would happily keep duplicates from a buggy replay —
        // and every by-id lookup (focus_id, status updates, upsert)
        // assumes uniqueness. Dedupe defensively: last occurrence wins
        // (newest data), first position kept (stable tab order).
        let mut deduped: Vec<SessionInfo> = Vec::with_capacity(items.len());
        let mut index_of: HashMap<String, usize> = HashMap::with_capacity(items.len());
        for s in items {
            if let Some(&i) = index_of.get(&s.id) {
                deduped[i] = s;
            } else {
                index_of.insert(s.id.clone(), deduped.len());
                deduped.push(s);
            }
        }
        self.items = deduped;
        self.focused = previously_focused
            .as_deref()
            .and_then(|id| self.items.iter().position(|s| s.id == id))
            .or_else(|| if self.items.is_empty() { None } else { Some(0) });
    }

    pub fn upsert(&mut self, session: SessionInfo) {
        if let Some(existing) = self.items.iter_mut().find(|s| s.id == session.id) {
            *existing = session;
        } else {
            self.items.push(session);
            if self.focused.is_none() {
                self.focused = Some(self.items.len() - 1);
            }
        }
    }

    pub fn remove(&mut self, session_id: &str) {
        self.items.retain(|s| s.id != session_id);
        self.focused = self.focused.and_then(|idx| {
            if self.items.is_empty() {
                None
            } else {
                Some(idx.min(self.items.len() - 1))
            }
        });
    }

    #[must_use]
    pub fn items(&self) -> &[SessionInfo] {
        &self.items
    }

    #[must_use]
    pub fn focused(&self) -> Option<&SessionInfo> {
        self.focused.and_then(|i| self.items.get(i))
    }

    #[must_use]
    pub fn focused_id(&self) -> Option<&str> {
        self.focused().map(|s| s.id.as_str())
    }

    #[must_use]
    pub fn focused_index(&self) -> Option<usize> {
        self.focused
    }

    pub fn focus_next(&mut self) {
        if self.items.is_empty() {
            self.focused = None;
            return;
        }
        self.focused = Some(self.focused.map_or(0, |i| (i + 1) % self.items.len()));
    }

    pub fn focus_prev(&mut self) {
        if self.items.is_empty() {
            self.focused = None;
            return;
        }
        self.focused = Some(
            self.focused
                .map_or(0, |i| (i + self.items.len() - 1) % self.items.len()),
        );
    }

    pub fn focus_id(&mut self, session_id: &str) {
        if let Some(idx) = self.items.iter().position(|s| s.id == session_id) {
            self.focused = Some(idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeoid_protocol::SessionStatus;

    fn mk(id: &str, name: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            name: name.into(),
            workdir: "/tmp".into(),
            status: SessionStatus::Idle,
            created_by: "me".into(),
            created_at: "2026-04-22T00:00:00Z".into(),
            attached_clients: 1,
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
            provider_id: None,
        }
    }

    #[test]
    fn replace_focuses_first_on_empty_start() {
        let mut list = SessionList::default();
        list.replace(vec![mk("a", "A"), mk("b", "B")]);
        assert_eq!(list.focused_id(), Some("a"));
    }

    #[test]
    fn replace_preserves_focus_by_id() {
        let mut list = SessionList::default();
        list.replace(vec![mk("a", "A"), mk("b", "B"), mk("c", "C")]);
        list.focus_next(); // b
        list.focus_next(); // c
        assert_eq!(list.focused_id(), Some("c"));

        // Daemon sends an updated list — focus should still land on "c".
        list.replace(vec![mk("a", "A"), mk("c", "C"), mk("b", "B")]);
        assert_eq!(list.focused_id(), Some("c"));
    }

    #[test]
    fn replace_falls_back_to_first_when_focused_id_removed() {
        let mut list = SessionList::default();
        list.replace(vec![mk("a", "A"), mk("b", "B")]);
        list.focus_id("b");
        list.replace(vec![mk("a", "A"), mk("c", "C")]);
        assert_eq!(list.focused_id(), Some("a"));
    }

    #[test]
    fn replace_empties_focus_when_empty() {
        let mut list = SessionList::default();
        list.replace(vec![mk("a", "A")]);
        list.replace(vec![]);
        assert_eq!(list.focused_id(), None);
    }

    #[test]
    fn replace_dedupes_duplicate_ids_last_wins_first_position() {
        // Ids are unique by daemon contract; a buggy replay must not
        // leave two tabs with the same id (by-id lookups — focus,
        // status changes, upsert — all assume uniqueness).
        let mut list = SessionList::default();
        let mut newer = mk("a", "A-newer");
        newer.status = SessionStatus::Working;
        list.replace(vec![mk("a", "A-old"), mk("b", "B"), newer]);

        assert_eq!(list.items().len(), 2, "duplicate id must collapse");
        assert_eq!(list.items()[0].id, "a", "first position kept");
        assert_eq!(list.items()[0].name, "A-newer", "newest data wins");
        assert!(matches!(list.items()[0].status, SessionStatus::Working));
        assert_eq!(list.items()[1].id, "b");
    }

    #[test]
    fn upsert_replaces_matching_id() {
        let mut list = SessionList::default();
        list.upsert(mk("a", "Original"));
        let mut updated = mk("a", "Renamed");
        updated.status = SessionStatus::Working;
        list.upsert(updated);
        assert_eq!(list.items().len(), 1);
        assert_eq!(list.items()[0].name, "Renamed");
        assert!(matches!(list.items()[0].status, SessionStatus::Working));
    }

    #[test]
    fn upsert_appends_and_focuses_when_list_was_empty() {
        let mut list = SessionList::default();
        list.upsert(mk("a", "A"));
        assert_eq!(list.focused_id(), Some("a"));
    }

    #[test]
    fn focus_next_wraps_around_end() {
        let mut list = SessionList::default();
        list.replace(vec![mk("a", "A"), mk("b", "B")]);
        list.focus_next();
        list.focus_next(); // wraps
        assert_eq!(list.focused_id(), Some("a"));
    }

    #[test]
    fn focus_prev_wraps_around_start() {
        let mut list = SessionList::default();
        list.replace(vec![mk("a", "A"), mk("b", "B"), mk("c", "C")]);
        list.focus_prev();
        assert_eq!(list.focused_id(), Some("c"));
    }

    #[test]
    fn focus_next_on_empty_is_noop() {
        let mut list = SessionList::default();
        list.focus_next();
        assert_eq!(list.focused_id(), None);
    }

    #[test]
    fn remove_shifts_focus_when_current_is_removed() {
        let mut list = SessionList::default();
        list.replace(vec![mk("a", "A"), mk("b", "B"), mk("c", "C")]);
        list.focus_id("c"); // last
        list.remove("c");
        assert_eq!(list.items().len(), 2);
        // Previously-focused index (2) no longer exists — should clamp to
        // the new last index (1).
        assert_eq!(list.focused_index(), Some(1));
        assert_eq!(list.focused_id(), Some("b"));
    }

    #[test]
    fn remove_drops_focus_when_list_becomes_empty() {
        let mut list = SessionList::default();
        list.replace(vec![mk("a", "A")]);
        list.remove("a");
        assert_eq!(list.focused_id(), None);
    }
}
