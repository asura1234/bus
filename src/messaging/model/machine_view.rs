//! Projection helpers for a client attached to another machine's coordinator.
use super::{Archived, BusState, Draft, History};
use std::sync::Arc;

impl BusState {
    pub(crate) fn machine_client_projection(&self) -> Self {
        let mut state = self.clone();
        state.visible_room = None;
        for room in state.rooms.values_mut() {
            room.draft = Draft::default();
            room.unread_count = 0;
            room.unseen_to_developer = false;
        }
        state
    }

    pub(crate) fn install_client_history(&mut self, items: Vec<Archived>) {
        let mut history = History::default();
        for item in items {
            history.insert(item);
        }
        self.history = Arc::new(history);
    }

    /// Only room identities survive a disconnected viewer's restart. This is
    /// a separate cache, never part of the saved SSH target list.
    pub(crate) fn machine_room_cache(&self) -> Self {
        let mut cached = Self {
            rooms: self.rooms.clone(),
            next_id: self.next_id,
            ..Self::default()
        };
        for room in cached.rooms.values_mut() {
            room.draft = Draft::default();
            room.unread_count = 0;
            room.unseen_to_developer = false;
            room.latest_prompt = None;
            room.notices.clear();
            room.latest_replies.clear();
            room.report_turns.clear();
            room.missing_provider_binaries.clear();
        }
        cached.visible_room = None;
        cached
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::model::{AgentRecipients, Author, Prompt, PromptId, RoomId};

    #[test]
    fn machine_history_window_has_per_room_and_byte_bounds() {
        let mut history = History::default();
        for room in [RoomId(1), RoomId(2)] {
            for id in 1..=20 {
                history.insert(Archived::Notice {
                    room,
                    prompt: Box::new(Prompt {
                        id: PromptId(room.0 * 100 + id),
                        author: Author::Bus,
                        text: "long archived message ".repeat(2048),
                        files: vec![],
                        recipient_ids: AgentRecipients::default(),
                        submitted_at_ms: id,
                        idle_agent_ids: Default::default(),
                        attention_origin: None,
                        in_reply_to: None,
                    }),
                });
            }
        }
        let window = history.client_window(3, 128 * 1024);
        assert!(serde_json::to_vec(&window).unwrap().len() < 128 * 1024);
        assert!(window.len() <= 6);
        assert!(!window.is_empty());
    }
}
