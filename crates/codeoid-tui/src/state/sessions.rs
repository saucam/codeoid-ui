//! Session list + focus state.

use codeoid_protocol::SessionInfo;

#[derive(Debug, Default)]
pub struct SessionList {
    items: Vec<SessionInfo>,
    focused: Option<usize>,
}

impl SessionList {
    pub fn replace(&mut self, items: Vec<SessionInfo>) {
        let previously_focused = self.focused_id().map(ToString::to_string);
        self.items = items;
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
        self.focused = Some(self.focused.map_or(0, |i| {
            (i + self.items.len() - 1) % self.items.len()
        }));
    }

    pub fn focus_id(&mut self, session_id: &str) {
        if let Some(idx) = self.items.iter().position(|s| s.id == session_id) {
            self.focused = Some(idx);
        }
    }
}
