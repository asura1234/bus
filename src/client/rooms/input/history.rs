//! Prompt recall, quoting and history-search navigation.
use super::super::{editor::Editor, BusUi, HistorySearch};
use crate::messaging::model::{RequestId, RoomId};
use crossterm::event::{KeyCode, KeyModifiers};

impl BusUi {
    pub fn quote(&mut self, request: RequestId) {
        let Some(room) = self.room else {
            return;
        };
        let Some((agent, text)) = super::super::history::reply(&self.snapshot.state, room, request)
        else {
            return;
        };
        let Some(name) = self.snapshot.state.agent(agent).map(|a| a.name.as_str()) else {
            return;
        };
        let quote = format!(
            "{name}: \"{}\"\n",
            text.replace('\\', "\\\\").replace('"', "\\\"")
        );
        if let Some(local) = self.locals.get_mut(&room) {
            // A leftover mouse selection must not be replaced by the quote.
            local.text.anchor = None;
            local.text.cursor = local.text.text.len();
            if !local.text.text.is_empty() && !local.text.text.ends_with('\n') {
                local.text.insert("\n");
            }
            local.text.insert(&quote);
        }
        self.notes_focus = false;
        self.recipient_menu = false;
        self.text_changed(room);
    }

    pub(super) fn history_entries(&self, room: RoomId) -> Vec<String> {
        let mut entries = self
            .locals
            .get(&room)
            .map(|local| local.recall.clone())
            .unwrap_or_default();
        for request in self
            .snapshot
            .state
            .requests()
            .filter(|request| request.room_id == room && !request.delivery_only())
        {
            if entries.last() != Some(&request.prompt.text) {
                entries.push(request.prompt.text.clone());
            }
        }
        if let Some(text) = self
            .snapshot
            .state
            .room(room)
            .and_then(|room| room.latest_prompt.as_ref())
            .map(|prompt| prompt.text.clone())
        {
            if entries.last() != Some(&text) {
                entries.push(text);
            }
        }
        entries
    }

    pub(super) fn history_or_move(&mut self, code: KeyCode) {
        let Some(room) = self.room else {
            return;
        };
        let Some(local) = self.locals.get(&room) else {
            return;
        };
        if code == KeyCode::Up && local.text.at_first_line() {
            self.history_older(room);
        } else if code == KeyCode::Down && local.text.at_last_line() {
            self.history_newer(room);
        } else if let Some(local) = self.locals.get_mut(&room) {
            local.composer_scroll = None;
            local.text.key(code, KeyModifiers::NONE);
        }
    }

    pub(super) fn history_older(&mut self, room: RoomId) {
        let entries = self.history_entries(room);
        if entries.is_empty() {
            return;
        }
        let Some(local) = self.locals.get_mut(&room) else {
            return;
        };
        let index = match local.history_index {
            None => {
                local.live_draft = Some(local.text.text.clone());
                entries.len() - 1
            }
            Some(0) => 0,
            Some(index) => index - 1,
        };
        local.history_index = Some(index);
        local.text = Editor::new(entries[index].clone());
        self.text_changed(room);
    }

    pub(super) fn history_newer(&mut self, room: RoomId) {
        let entries = self.history_entries(room);
        let Some(local) = self.locals.get_mut(&room) else {
            return;
        };
        match local.history_index {
            None => {}
            Some(index) if index + 1 < entries.len() => {
                local.history_index = Some(index + 1);
                local.text = Editor::new(entries[index + 1].clone());
                self.text_changed(room);
            }
            Some(_) => {
                local.history_index = None;
                let draft = local.live_draft.take().unwrap_or_default();
                local.text = Editor::new(draft);
                self.text_changed(room);
            }
        }
    }

    pub(super) fn cycle_history_search(&mut self) {
        let Some(room) = self.room else {
            return;
        };
        if self.history_entries(room).is_empty() {
            return;
        }
        match &mut self.history_search {
            None => {
                let live_draft = self
                    .locals
                    .get(&room)
                    .map(|local| local.text.text.clone())
                    .unwrap_or_default();
                self.history_search = Some(HistorySearch {
                    query: String::new(),
                    selected: 0,
                    live_draft,
                });
            }
            Some(search) => search.selected = search.selected.saturating_add(1),
        }
        self.apply_search(room);
    }

    pub(in crate::client::rooms) fn filtered_history(
        &self,
        room: RoomId,
        query: &str,
    ) -> Vec<String> {
        let query = query.to_lowercase();
        self.history_entries(room)
            .into_iter()
            .rev()
            .filter(|text| query.is_empty() || text.to_lowercase().contains(&query))
            .collect()
    }

    pub(super) fn apply_search(&mut self, room: RoomId) {
        let Some(search) = &self.history_search else {
            return;
        };
        let matches = self.filtered_history(room, &search.query);
        if matches.is_empty() {
            return;
        }
        let selected = search.selected % matches.len();
        let text = matches[selected].clone();
        if let Some(search) = &mut self.history_search {
            search.selected = selected;
        }
        if let Some(local) = self.locals.get_mut(&room) {
            local.text = Editor::new(text);
            local.history_index = None;
        }
        self.text_changed(room);
    }

    pub(super) fn cancel_search(&mut self) {
        let Some(search) = self.history_search.take() else {
            return;
        };
        let Some(room) = self.room else {
            return;
        };
        if let Some(local) = self.locals.get_mut(&room) {
            local.text = Editor::new(search.live_draft);
            local.history_index = None;
            local.live_draft = None;
        }
        self.text_changed(room);
    }

    pub(super) fn search_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let Some(room) = self.room else {
            return;
        };
        match (code, modifiers) {
            (KeyCode::Esc, _) => self.cancel_search(),
            (KeyCode::Enter, _) => self.history_search = None,
            (KeyCode::Char('r' | 'R'), KeyModifiers::CONTROL) => {
                if let Some(search) = &mut self.history_search {
                    search.selected = search.selected.saturating_add(1);
                }
                self.apply_search(room);
            }
            (KeyCode::Backspace, _) => {
                if let Some(search) = &mut self.history_search {
                    search.query.pop();
                    search.selected = 0;
                }
                self.apply_search(room);
            }
            (KeyCode::Char(c), modifiers)
                if !modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                if let Some(search) = &mut self.history_search {
                    search.query.push(c);
                    search.selected = 0;
                }
                self.apply_search(room);
            }
            _ => {}
        }
    }
}
