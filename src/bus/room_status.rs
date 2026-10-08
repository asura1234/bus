use super::model::{Agent, BusState, RoomId, RuntimeStatus};

impl Agent {
    /// The status Bus shows for this agent: a visible choice dialog blocks it
    /// whatever its provider reports (Codex reports Idle while it waits on an
    /// approval). Agent rows and room status both use it, so they agree.
    pub(crate) fn shown_status(&self) -> RuntimeStatus {
        if self.dialog {
            RuntimeStatus::Blocked
        } else {
            self.status
        }
    }
}

impl BusState {
    /// A room's activity from its agents' shown status, in precedence order:
    /// `Blocked` if any agent is blocked (including on a dialog) or
    /// Unavailable, else `Working` if any works, else `Idle` (including a room
    /// without agents). An Unavailable agent counts as blocked because its
    /// provider is not running, so the room cannot progress without help; an
    /// agent being deleted does not count.
    pub(crate) fn room_status(&self, room: RoomId) -> RuntimeStatus {
        let mut status = RuntimeStatus::Idle;
        for agent in self.agents().filter(|agent| agent.room_id == room) {
            match agent.shown_status() {
                RuntimeStatus::Blocked => return RuntimeStatus::Blocked,
                RuntimeStatus::Unavailable if !agent.deletion_pending => {
                    return RuntimeStatus::Blocked
                }
                RuntimeStatus::Working => status = RuntimeStatus::Working,
                _ => {}
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

    #[test]
    fn a_visible_dialog_blocks_the_agent_row_and_the_room_alike() {
        let mut state = BusState::default();
        let room = state.create_room("work").unwrap();
        let codex = state
            .create_agent(room, "codex", Provider::Codex, "/repo".into(), None)
            .unwrap();
        let other = state
            .create_agent(room, "other", Provider::Codex, "/repo".into(), None)
            .unwrap();
        state
            .observe_status(other, RuntimeStatus::Working, 1)
            .unwrap();

        // Codex reports Idle while its approval dialog is on screen.
        state.observe_status(codex, RuntimeStatus::Idle, 1).unwrap();
        state.observe_dialog(codex, true).unwrap();
        assert_eq!(
            state.agent(codex).unwrap().shown_status(),
            RuntimeStatus::Blocked
        );
        assert_eq!(state.room_status(room), RuntimeStatus::Blocked);

        // The dialog closes: row and room follow on the same state change.
        state.observe_dialog(codex, false).unwrap();
        assert_eq!(
            state.agent(codex).unwrap().shown_status(),
            RuntimeStatus::Idle
        );
        assert_eq!(state.room_status(room), RuntimeStatus::Working);

        // Deleting a blocked agent unblocks the room.
        state
            .observe_status(codex, RuntimeStatus::Blocked, 1)
            .unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Blocked);
        state.delete_agent(codex).unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Working);
    }

    #[test]
    fn an_unavailable_agent_blocks_its_room_over_a_working_one() {
        let mut state = BusState::default();
        let room = state.create_room("work").unwrap();
        let agent = |state: &mut BusState, name| {
            state
                .create_agent(room, name, Provider::Codex, "/repo".into(), None)
                .unwrap()
        };
        let (gone, busy) = (agent(&mut state, "gone"), agent(&mut state, "busy"));
        state
            .observe_status(busy, RuntimeStatus::Working, 1)
            .unwrap();
        state.observe_status(gone, RuntimeStatus::Idle, 1).unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Working);

        // Its provider exits or is replaced: the room needs help.
        state
            .observe_status(gone, RuntimeStatus::Unavailable, 2)
            .unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Blocked);

        // Relaunched, the room is back to its agents' activity.
        state.observe_status(gone, RuntimeStatus::Idle, 3).unwrap();
        assert_eq!(state.room_status(room), RuntimeStatus::Working);
    }
}
