use super::{BusUi, Effect};
use crate::messaging::coordinator::BusCommand;

impl BusUi {
    pub(super) fn effective_history_filters(
        &self,
        room: crate::messaging::model::RoomId,
    ) -> Option<crate::messaging::model::RoomHistoryFilters> {
        let pending = self
            .pending
            .iter()
            .rev()
            .find_map(|pending| match pending.command {
                BusCommand::SetRoomHistoryFilters(id, filters)
                    if id == room && !matches!(&pending.result, Some(Err(_))) =>
                {
                    Some(filters)
                }
                _ => None,
            });
        pending.or_else(|| {
            self.snapshot
                .state
                .room(room)
                .map(|room| room.history_filters)
        })
    }

    pub(super) fn toggle_history_filter(&mut self, to_you: bool) {
        let Some(room) = self.room else { return };
        let Some(mut filters) = self.effective_history_filters(room) else {
            return;
        };
        if to_you {
            filters.to_you = !filters.to_you;
        } else {
            filters.from_you = !filters.from_you;
        }
        self.clear_selection();
        self.queue(
            BusCommand::SetRoomHistoryFilters(room, filters),
            Effect::None,
        );
    }
}
