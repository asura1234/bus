use super::{BusUi, Effect};
use crate::messaging::coordinator::BusCommand;

impl BusUi {
    pub(super) fn toggle_history_filter(&mut self, to_you: bool) {
        let Some(room) = self.room else {
            return;
        };
        let Some(mut filters) = self
            .snapshot
            .state
            .room(room)
            .map(|room| room.history_filters)
        else {
            return;
        };
        if to_you {
            filters.to_you = !filters.to_you;
        } else {
            filters.from_you = !filters.from_you;
        }
        self.queue(
            BusCommand::SetRoomHistoryFilters(room, filters),
            Effect::None,
        );
    }
}
