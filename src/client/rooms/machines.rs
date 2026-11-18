//! Machine identity and navigation live in the client, never in a room's durable model.
use super::BusUi;
use crate::messaging::{
    coordinator::BusHandle,
    model::{RoomId, RuntimeStatus},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::client) struct MachineRoomRow {
    pub machine: String,
    pub room: RoomId,
    pub name: String,
    pub status: RuntimeStatus,
    pub active_agents: usize,
    pub total_agents: usize,
    pub unread: bool,
}

impl BusUi {
    pub(super) fn poll_machine_availability(&mut self) -> bool {
        if self.remote_machine && !self.coordinator_connected() && self.machine_available {
            self.set_machine_available(false);
            true
        } else {
            false
        }
    }

    pub(in crate::client) fn new_remote(
        handle: BusHandle,
        label: String,
        target: &str,
        sound: &crate::utils::config::SoundConfig,
    ) -> Result<Self, String> {
        let snapshot = handle.snapshot().ok_or("Machine snapshot unavailable")?;
        let mut bus = Self::new(snapshot);
        bus.machine_available = handle.is_connected();
        bus.handle = Some(handle);
        bus.set_machine(label.clone(), true);
        bus.owns_coordinator = false;
        bus.seed_first_room = false;
        bus.dashboard_root = None;
        bus.sound_config = Some(sound.clone());
        bus.history.enable_background()?;
        if let Some(root) = crate::utils::env::bus_data_dir() {
            bus.enable_client_state(&root, &format!("ssh:{target}"))?;
        }
        Ok(bus)
    }

    pub(in crate::client) fn set_machine(&mut self, label: String, remote: bool) {
        self.machine_label = label;
        self.remote_machine = remote;
        if remote {
            self.graphics = None;
            self.thumbnails.set_cell(None);
        }
        self.full_repaint = true;
    }

    pub(in crate::client) fn set_machine_available(&mut self, available: bool) {
        if self.machine_available != available {
            self.machine_available = available;
            self.full_repaint = true;
        }
    }

    pub(in crate::client) fn coordinator_connected(&self) -> bool {
        self.handle.as_ref().is_some_and(BusHandle::is_connected)
    }

    pub(in crate::client) fn owns_coordinator(&self) -> bool {
        self.owns_coordinator
    }

    pub(in crate::client) fn machine_rows(&self) -> Vec<MachineRoomRow> {
        self.snapshot
            .state
            .rooms()
            .map(|room| {
                let agents = self
                    .snapshot
                    .state
                    .agents()
                    .filter(|agent| agent.room_id == room.id);
                let (active_agents, total_agents) = if self.machine_available {
                    agents.fold((0, 0), |(active, total), agent| {
                        (
                            active
                                + usize::from(matches!(
                                    agent.shown_status(),
                                    RuntimeStatus::Working | RuntimeStatus::Blocked
                                )),
                            total + 1,
                        )
                    })
                } else {
                    (0, 0)
                };
                MachineRoomRow {
                    machine: self.machine_label.clone(),
                    room: room.id,
                    name: room.name.clone(),
                    status: if self.machine_available {
                        self.snapshot.state.room_status(room.id)
                    } else {
                        RuntimeStatus::Unavailable
                    },
                    active_agents,
                    total_agents,
                    unread: self.room_unread(room.id),
                }
            })
            .collect()
    }

    pub(in crate::client) fn set_other_machine_rooms(
        &mut self,
        rooms: Vec<MachineRoomRow>,
    ) -> bool {
        if self.other_machine_rooms != rooms {
            self.other_machine_rooms = rooms;
            self.full_repaint = true;
            return true;
        }
        false
    }

    pub(in crate::client) fn take_machine_switch(&mut self) -> Option<(String, RoomId)> {
        self.machine_switch.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::{
        coordinator::BusSnapshot,
        model::{BusState, Provider},
    };
    use std::sync::Arc;

    #[test]
    fn multi_machine_rooms_distinguish_empty_live_and_down_rooms() {
        let mut state = BusState::default();
        state.ensure_master_room();
        let room = state.create_room("same-name").unwrap();
        let mut ui = BusUi::new(Arc::new(BusSnapshot {
            state,
            revision: 0,
            last_command_id: 0,
            error: None,
        }));
        ui.set_machine("laptop".into(), true);
        let row = ui
            .machine_rows()
            .into_iter()
            .find(|r| r.room == room)
            .unwrap();
        assert_eq!(row.status, RuntimeStatus::Idle);
        assert_eq!((row.active_agents, row.total_agents), (0, 0));
        let mut next = (*ui.snapshot).clone();
        next.state
            .create_agent(room, "worker", Provider::Codex, "/remote".into(), None)
            .unwrap();
        ui.receive_snapshot(Arc::new(next));
        ui.set_machine_available(false);
        let rows = ui.machine_rows();
        assert!(rows.iter().all(|r| r.status == RuntimeStatus::Unavailable
            && r.total_agents == 0
            && r.active_agents == 0));
        assert!(rows.iter().any(|r| r.name == "MASTER"));
        assert!(rows
            .iter()
            .any(|r| r.name == "same-name" && r.machine == "laptop"));
    }
    #[test]
    fn multi_machine_foreign_room_hit_keeps_machine_identity() {
        let mut state = BusState::default();
        state.ensure_master_room();
        let room = state.create_room("same-name").unwrap();
        let mut ui = BusUi::new(Arc::new(BusSnapshot {
            state,
            revision: 0,
            last_command_id: 0,
            error: None,
        }));
        ui.set_other_machine_rooms(vec![MachineRoomRow {
            machine: "laptop".into(),
            room,
            name: "same-name".into(),
            status: RuntimeStatus::Unavailable,
            active_agents: 0,
            total_agents: 0,
            unread: false,
        }]);
        ui.compute_view(160, 48);
        let action = super::super::render::Action::MachineRoom("laptop".into(), room);
        assert!(ui.view.hits.iter().any(|hit| hit.action == action));
        ui.action(action);
        assert_eq!(ui.take_machine_switch(), Some(("laptop".into(), room)));
        assert!(ui.take_machine_switch().is_none());
    }
}
