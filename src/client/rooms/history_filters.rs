use super::{BusUi, Effect};
use crate::messaging::coordinator::BusCommand;

impl BusUi {
    pub(super) fn toggle_history_filter(&mut self, to_you: bool) {
        let Some(room) = self.room else {
            return;
        };
        let pending = self
            .pending
            .iter()
            .rev()
            .find_map(|pending| match pending.command {
                BusCommand::SetRoomHistoryFilters(id, filters)
                    if id == room && pending.result.is_none() =>
                {
                    Some(filters)
                }
                _ => None,
            });
        let Some(mut filters) = pending.or_else(|| {
            self.snapshot
                .state
                .room(room)
                .map(|room| room.history_filters)
        }) else {
            return;
        };
        if to_you {
            filters.to_you = !filters.to_you;
        } else {
            filters.from_you = !filters.from_you;
        }
        self.clear_selection();
        if let Some(search) = &mut self.chat_search {
            search.jump_to_newest = true;
        }
        self.queue(
            BusCommand::SetRoomHistoryFilters(room, filters),
            Effect::None,
        );
    }
}
