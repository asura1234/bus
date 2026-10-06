use super::model::{BusState, RoomId, RuntimeStatus};

impl BusState {
    /// A room's activity from its agents: `Blocked` if any agent is blocked or
    /// waits on a dialog, else `Working` if any works, else `Idle` (including a
    /// room without agents).
    pub(crate) fn room_status(&self, room: RoomId) -> RuntimeStatus {
        let mut status = RuntimeStatus::Idle;
        for agent in self.agents().filter(|agent| agent.room_id == room) {
            if agent.dialog || agent.status == RuntimeStatus::Blocked {
                return RuntimeStatus::Blocked;
            }
            if agent.status == RuntimeStatus::Working {
                status = RuntimeStatus::Working;
            }
        }
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::model::Provider;

    #[test]
    fn room_status_prefers_blocked_then_working_then_idle() {
        let mut state = BusState::default();
        let master = state.ensure_master_room();
        let room = state.create_room("work").unwrap();
        let empty = state.create_room("empty").unwrap();
        let agent = |state: &mut BusState, room, name| {
            state
                .create_agent(room, name, Provider::Codex, "/repo".into(), None)
                .unwrap()
        };
        let first = agent(&mut state, room, "first");
        let second = agent(&mut state, room, "second");
        let orchestrator = agent(&mut state, master, "orch");

        assert_eq!(state.room_status(empty), RuntimeStatus::Idle);
        state.observe_status(first, RuntimeStatus::Idle, 1).unwrap();
        state
            .observe_status(second, RuntimeStatus::Idle, 1)
            .unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Idle);

        state
            .observe_status(first, RuntimeStatus::Working, 1)
            .unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Working);

        state
            .observe_status(second, RuntimeStatus::Blocked, 1)
            .unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Blocked);

        // A dialog blocks the room even while its agent reports working.
        state
            .observe_status(second, RuntimeStatus::Working, 1)
            .unwrap();
        state.observe_dialog(second, true).unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Blocked);

        // MASTER follows its own orchestrator agents only.
        assert_ne!(state.room_status(master), RuntimeStatus::Blocked);
        state
            .observe_status(orchestrator, RuntimeStatus::Working, 1)
            .unwrap();
        assert_eq!(state.room_status(master), RuntimeStatus::Working);
        assert_eq!(state.room_status(empty), RuntimeStatus::Idle);
    }
}
