//! Independent room views and pane transports for saved SSH machines.
//!
//! IDs are never flattened: each machine owns a complete shell projection and its
//! connection generation. Switching the view swaps both together.
use super::{
    client_shell_resize_message, endpoint, endpoint_commands, handshake, shell,
    start_endpoint_transport, ClientError, ClientLoopDriver, ClientLoopEvent, ClientMessage,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;
use std::time::Instant;
use std::{io, time::Duration};

const LOCAL: &str = "local";
const RECONNECT_DELAY: Duration = Duration::from_secs(3);

pub(super) struct MachineEndpoint {
    shell: shell::ClientShellState,
    connection: endpoint::ServerConnection,
    commands: endpoint_commands::EndpointCommands,
    connection_id: u64,
}

struct Connected {
    label: String,
    target: String,
    connection_id: u64,
    result: Result<
        (
            crate::platform::ipc::LocalStream,
            crate::messaging::machines::TerminalConnection,
            handshake::HandshakeResult,
        ),
        String,
    >,
}

pub(super) struct MachineClients {
    active: String,
    restore: Option<String>,
    inactive: BTreeMap<String, MachineEndpoint>,
    targets: BTreeMap<String, String>,
    connecting: BTreeSet<String>,
    retry_at: BTreeMap<String, Instant>,
    next_id: u64,
    refresh_at: Instant,
    sender: mpsc::Sender<Connected>,
    receiver: mpsc::Receiver<Connected>,
    event_tx: Option<crate::utils::perf::queue::Sender<ClientLoopEvent>>,
}

impl Default for MachineClients {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            active: LOCAL.into(),
            restore: None,
            inactive: BTreeMap::new(),
            targets: BTreeMap::new(),
            connecting: BTreeSet::new(),
            retry_at: BTreeMap::new(),
            next_id: 1,
            refresh_at: Instant::now(),
            sender,
            receiver,
            event_tx: None,
        }
    }
}

impl ClientLoopDriver {
    pub(super) fn initialize_machines(
        &mut self,
        event_tx: &crate::utils::perf::queue::Sender<ClientLoopEvent>,
    ) {
        if self
            .state
            .shell
            .as_ref()
            .is_some_and(|shell| shell.has_bus())
        {
            self.machines.event_tx = Some(event_tx.clone());
            self.machines.restore = crate::utils::env::bus_data_dir()
                .and_then(|root| crate::client::rooms::BusUi::saved_machine(&root))
                .filter(|label| label != LOCAL);
        }
    }

    pub(super) fn is_remote(&self) -> bool {
        self.machines.active != LOCAL
    }

    /// Handles a failed remote write once. There is no input or mutation replay.
    pub(super) fn remote_failure(&mut self, error: ClientError) -> Result<bool, ClientError> {
        if !self.is_remote() {
            return Err(error);
        }
        tracing::warn!(machine = %self.machines.active, %error, "remote pane connection failed");
        self.write_stream.fail(io::Error::other(error.to_string()));
        self.write_stream.take_failure();
        if let Some(shell) = self.state.shell.as_mut() {
            shell.reset_endpoint_projection();
            shell.snapshot = None;
            if let Some(bus) = shell.bus.as_mut() {
                bus.set_machine_available(false);
            }
        }
        self.machines.retry_at.insert(
            self.machines.active.clone(),
            Instant::now() + RECONNECT_DELAY,
        );
        self.state.request_repaint();
        if let Some(frame) =
            self.state.shell.as_mut().and_then(|shell| {
                shell.compose(self.state.reported_size.0, self.state.reported_size.1)
            })
        {
            self.state.present_frame(frame);
        }
        Ok(false)
    }

    pub(super) fn handle_background_message(
        &mut self,
        connection_id: u64,
        message: Box<crate::protocol::wire::ServerMessage>,
    ) {
        use crate::protocol::wire::ServerMessage;
        let Some(slot) = self
            .machines
            .inactive
            .values_mut()
            .find(|slot| slot.connection_id == connection_id)
        else {
            return;
        };
        if !slot.connection.is_connected() {
            return;
        }
        match *message {
            ServerMessage::EndpointControl { kind, data } => {
                match endpoint::decode_endpoint_control(&kind, &data) {
                    Ok(endpoint::EndpointControlMessage::Snapshot(snapshot)) => {
                        slot.shell.install_snapshot(snapshot)
                    }
                    Ok(endpoint::EndpointControlMessage::Ignored) => {}
                    Err(error) => slot.connection.fail(io::Error::other(error)),
                }
            }
            ServerMessage::PaneSurface(surface) => slot.shell.set_pane_surface(surface),
            ServerMessage::ClientShellEndpointResponseChunk {
                boot_id,
                request_id,
                final_chunk,
                data,
            } => {
                if let Some(completed) =
                    slot.commands
                        .receive_chunk(&boot_id, &request_id, final_chunk, data)
                {
                    // Focus acknowledgements may arrive just after switching away.
                    let _ = slot.shell.handle_endpoint_result(
                        &completed.boot_id,
                        &completed.request_id,
                        completed.result,
                    );
                }
            }
            ServerMessage::ServerShutdown { .. } => slot.connection.fail(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "remote Bus stopped",
            )),
            _ => {} // Inactive machines never write into this client's terminal.
        }
    }

    pub(super) fn background_disconnected(&mut self, connection_id: u64) {
        if let Some(slot) = self
            .machines
            .inactive
            .values_mut()
            .find(|slot| slot.connection_id == connection_id)
        {
            slot.connection.fail(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "machine disconnected",
            ));
        }
    }

    pub(super) fn switch_machine(
        &mut self,
        label: &str,
        room: crate::messaging::model::RoomId,
    ) -> bool {
        if label == self.machines.active {
            return false;
        }
        let Some(mut next) = self.machines.inactive.remove(label) else {
            return false;
        };
        let Some(mut previous_shell) = self.state.shell.take() else {
            self.machines.inactive.insert(label.into(), next);
            return false;
        };
        let _ = self
            .write_stream
            .send(&ClientMessage::ClientShellFocus { focused: false });
        previous_shell.outer_focused = Some(false);
        if let Some(bus) = previous_shell.bus.as_mut() {
            bus.set_client_focused(false);
        }
        let previous = MachineEndpoint {
            shell: previous_shell,
            connection: std::mem::replace(&mut self.write_stream, next.connection),
            commands: std::mem::replace(&mut self.endpoint_commands, next.commands),
            connection_id: std::mem::replace(&mut self.connection_id, next.connection_id),
        };
        if let Some(bus) = next.shell.bus.as_mut() {
            bus.open_room(room);
        }
        next.shell.outer_focused = Some(true);
        if let Some(bus) = next.shell.bus.as_mut() {
            bus.set_client_focused(true);
        }
        next.shell.invalidate_pane_surface();
        self.state.shell = Some(next.shell);
        let old = std::mem::replace(&mut self.machines.active, label.into());
        self.machines.inactive.insert(old, previous);
        if let Some(root) = crate::utils::env::bus_data_dir() {
            if let Err(error) = crate::client::rooms::BusUi::save_active_machine(&root, label) {
                tracing::warn!(%error, "could not save selected machine");
            }
        }
        self.state.pane_keyboard_report_all = false;
        self.state.output.invalidate();
        self.state.request_repaint();
        if self.write_stream.is_connected() {
            let _ = self
                .write_stream
                .send(&ClientMessage::ClientShellFocus { focused: true });
            if let Some(shell) = self.state.shell.as_ref() {
                let resize = client_shell_resize_message(
                    shell,
                    self.state.reported_size.0,
                    self.state.reported_size.1,
                    self.state.reported_cell_size.0,
                    self.state.reported_cell_size.1,
                    self.state.pixel_geometry_exact,
                );
                let _ = self.write_stream.send(&resize);
            }
        }
        true
    }

    pub(super) fn tick_machines(&mut self, now: Instant) -> bool {
        if self.machines.event_tx.is_none() {
            return false;
        }
        let mut changed = false;
        changed |= self.refresh_machine_targets(now);
        changed |= self.receive_machine_connections(now);
        changed |= self.tick_background_machines(now);
        changed |= self.apply_machine_switches(now);
        changed |= self.update_machine_rows(now);
        changed |= self.reconnect_machine_panes(now);
        changed
    }

    fn refresh_machine_targets(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if now >= self.machines.refresh_at {
            self.machines.refresh_at = now + Duration::from_secs(2);
            match crate::messaging::machines::load_targets() {
                Ok(targets) => {
                    let targets: BTreeMap<_, _> = targets
                        .into_iter()
                        .filter(|target| target.enabled)
                        .map(|target| (target.label, target.target))
                        .collect();
                    let removed: Vec<_> = self
                        .machines
                        .targets
                        .iter()
                        .filter(|(label, target)| targets.get(*label) != Some(*target))
                        .map(|(label, _)| label.clone())
                        .collect();
                    for label in removed {
                        if self.machines.active == label {
                            let room = self
                                .machines
                                .inactive
                                .get(LOCAL)
                                .and_then(|slot| slot.shell.bus.as_ref())
                                .and_then(|bus| {
                                    bus.room
                                        .or_else(|| bus.machine_rows().first().map(|row| row.room))
                                });
                            if let Some(room) = room {
                                self.switch_machine(LOCAL, room);
                            }
                        }
                        self.machines.inactive.remove(&label);
                        self.machines.retry_at.remove(&label);
                        changed = true;
                    }
                    for (label, target) in &targets {
                        if self.machines.active == *label
                            || self.machines.inactive.contains_key(label)
                        {
                            continue;
                        }
                        let created =
                            crate::messaging::coordinator::BusHandle::connect_remote_named(
                                label.clone(),
                                target.clone(),
                            )
                            .and_then(|handle| {
                                crate::client::rooms::BusUi::new_remote(
                                    handle,
                                    label.clone(),
                                    target,
                                    &self.state.sound_config,
                                )
                            });
                        match created {
                            Ok(bus) => {
                                let config = crate::utils::config::Config::load().config;
                                let mut shell = shell::ClientShellState::new(
                                    shell::ClientShellConfig::from_config(&config),
                                );
                                shell.bus = Some(bus);
                                shell.outer_focused = Some(false);
                                shell.set_graphics_cell_size(
                                    self.state.reported_cell_size.0,
                                    self.state.reported_cell_size.1,
                                );
                                // Remote attachment files must never be opened on this machine.
                                shell.set_bus_kitty_graphics(false);
                                self.machines.inactive.insert(
                                    label.clone(),
                                    MachineEndpoint {
                                        shell,
                                        connection: endpoint::ServerConnection::disconnected(),
                                        commands: Default::default(),
                                        connection_id: u64::MAX,
                                    },
                                );
                                changed = true;
                            }
                            Err(error) => {
                                tracing::warn!(machine = %label, %error, "remote room view unavailable")
                            }
                        }
                    }
                    self.machines.targets = targets;
                }
                Err(error) => tracing::warn!(%error, "saved machines unavailable"),
            }
        }
        changed
    }

    fn receive_machine_connections(&mut self, now: Instant) -> bool {
        let mut changed = false;
        while let Ok(connected) = self.machines.receiver.try_recv() {
            self.machines.connecting.remove(&connected.label);
            if self.machines.targets.get(&connected.label) != Some(&connected.target) {
                continue;
            }
            let (stream, lifetime, handshake) = match connected.result {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::debug!(machine = %connected.label, %error, "SSH reconnect failed");
                    self.machines
                        .retry_at
                        .insert(connected.label, now + RECONNECT_DELAY);
                    continue;
                }
            };
            let Some(event_tx) = self.machines.event_tx.as_ref() else {
                continue;
            };
            let transport = match start_endpoint_transport(
                stream,
                lifetime,
                event_tx,
                crate::protocol::wire::MAX_GRAPHICS_FRAME_SIZE,
                connected.connection_id,
            ) {
                Ok(transport) => transport,
                Err(error) => {
                    tracing::warn!(%error, "SSH reader failed");
                    continue;
                }
            };
            let mut connection = endpoint::ServerConnection::new(transport);
            let active = self.machines.active == connected.label;
            let _ = connection.send(&ClientMessage::ClientShellFocus { focused: active });
            if active {
                self.write_stream = connection;
                self.connection_id = connected.connection_id;
                self.endpoint_commands = Default::default();
                if let Some(shell) = self.state.shell.as_mut() {
                    shell.set_bus_server_build(handshake.server.as_ref());
                }
            } else if let Some(slot) = self.machines.inactive.get_mut(&connected.label) {
                slot.connection = connection;
                slot.connection_id = connected.connection_id;
                slot.commands = Default::default();
                slot.shell.set_bus_server_build(handshake.server.as_ref());
            }
            changed = true;
        }
        changed
    }

    fn tick_background_machines(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for (label, slot) in &mut self.machines.inactive {
            if let Some(error) = slot.connection.take_failure() {
                tracing::debug!(machine = %label, %error, "background machine disconnected");
                slot.shell.reset_endpoint_projection();
                slot.shell.snapshot = None;
                self.machines
                    .retry_at
                    .insert(label.clone(), now + RECONNECT_DELAY);
                changed = true;
            }
            if let Some(bus) = slot.shell.bus.as_mut() {
                bus.set_client_focused(false);
                bus.set_machine_available(
                    slot.connection.is_connected() && bus.coordinator_connected(),
                );
                changed |= bus.tick();
            }
        }
        if let Some(bus) = self
            .state
            .shell
            .as_mut()
            .and_then(|shell| shell.bus.as_mut())
        {
            if self.machines.active != LOCAL {
                bus.set_machine_available(
                    self.write_stream.is_connected() && bus.coordinator_connected(),
                );
            }
        }
        changed
    }

    fn apply_machine_switches(&mut self, _now: Instant) -> bool {
        let mut changed = false;
        if self.is_remote() {
            let quit_room = self
                .machines
                .inactive
                .get(LOCAL)
                .and_then(|slot| slot.shell.bus.as_ref())
                .filter(|bus| bus.exit_ready)
                .and_then(|bus| {
                    bus.room
                        .or_else(|| bus.machine_rows().first().map(|row| row.room))
                });
            if let Some(room) = quit_room {
                changed |= self.switch_machine(LOCAL, room);
            }
        }
        let pending = self
            .state
            .shell
            .as_mut()
            .and_then(|shell| shell.bus.as_mut())
            .and_then(|bus| bus.take_machine_switch());
        if let Some((label, room)) = pending {
            self.machines.restore = None;
            changed |= self.switch_machine(&label, room);
        }
        if let Some(label) = self.machines.restore.clone() {
            if let Some(room) = self
                .machines
                .inactive
                .get(&label)
                .and_then(|slot| slot.shell.bus.as_ref())
                .and_then(|bus| bus.room)
            {
                self.machines.restore = None;
                changed |= self.switch_machine(&label, room);
            }
        }
        changed
    }

    fn update_machine_rows(&mut self, _now: Instant) -> bool {
        let mut changed = false;
        let mut rows = Vec::new();
        if let Some(bus) = self
            .state
            .shell
            .as_ref()
            .and_then(|shell| shell.bus.as_ref())
        {
            rows.extend(bus.machine_rows());
        }
        for slot in self.machines.inactive.values() {
            if let Some(bus) = slot.shell.bus.as_ref() {
                rows.extend(bus.machine_rows());
            }
        }
        if let Some(bus) = self
            .state
            .shell
            .as_mut()
            .and_then(|shell| shell.bus.as_mut())
        {
            changed |= bus.set_other_machine_rooms(
                rows.into_iter()
                    .filter(|row| row.machine != self.machines.active)
                    .collect(),
            );
        }
        changed
    }

    fn reconnect_machine_panes(&mut self, now: Instant) -> bool {
        let targets = self.machines.targets.clone();
        for (label, target) in targets {
            let connected = if self.machines.active == label {
                self.write_stream.is_connected()
            } else {
                self.machines
                    .inactive
                    .get(&label)
                    .is_some_and(|slot| slot.connection.is_connected())
            };
            if connected
                || self.machines.connecting.contains(&label)
                || self
                    .machines
                    .retry_at
                    .get(&label)
                    .is_some_and(|at| now < *at)
            {
                continue;
            }
            let Some(shell) = self.state.shell.as_ref() else {
                continue;
            };
            let size = shell.surface_size(self.state.reported_size.0, self.state.reported_size.1);
            let id = self.machines.next_id;
            self.machines.next_id += 1;
            self.machines.connecting.insert(label.clone());
            let sender = self.machines.sender.clone();
            let cell = self.state.reported_cell_size;
            let mouse = self.state.mouse_capture_active;
            let spawned_label = label.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("bus-ssh-connect".into())
                .spawn(move || {
                    let result = crate::messaging::machines::connect_terminal(&target).and_then(
                        |(mut stream, lifetime)| {
                            // Pixel geometry and direct graphics are deliberately not negotiated across SSH.
                            let handshake = handshake::do_handshake(
                                &mut stream,
                                cell.0,
                                cell.1,
                                false,
                                size,
                                false,
                                mouse,
                            )
                            .map_err(|error| error.to_string())?;
                            Ok((stream, lifetime, handshake))
                        },
                    );
                    let _ = sender.send(Connected {
                        label: spawned_label,
                        target,
                        connection_id: id,
                        result,
                    });
                })
            {
                self.machines.connecting.remove(&label);
                self.machines.retry_at.insert(label, now + RECONNECT_DELAY);
                tracing::warn!(%error, "could not start SSH connect");
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::model::RoomId;

    #[test]
    fn equal_room_ids_on_two_machines_keep_their_own_shell_and_input_transport() {
        let (mut local, local_sent) = super::super::tests::driver(120);
        let (mut remote, remote_sent) = super::super::tests::driver(120);
        remote
            .state
            .shell
            .as_mut()
            .unwrap()
            .bus
            .as_mut()
            .unwrap()
            .set_machine("remote".into(), true);
        local.machines.inactive.insert(
            "remote".into(),
            MachineEndpoint {
                shell: remote.state.shell.take().unwrap(),
                connection: remote.write_stream,
                commands: remote.endpoint_commands,
                connection_id: 7,
            },
        );
        assert!(local.switch_machine("remote", RoomId(1)));
        assert_eq!(local.connection_id, 7);
        local
            .write_stream
            .send(&ClientMessage::ClientShellFocus { focused: true })
            .unwrap();
        assert!(matches!(
            local_sent.lock().unwrap().last(),
            Some(ClientMessage::ClientShellFocus { focused: false })
        ));
        assert!(matches!(
            remote_sent.lock().unwrap().last(),
            Some(ClientMessage::ClientShellFocus { focused: true })
        ));
        assert!(local.switch_machine(LOCAL, RoomId(1)));
        assert_eq!(local.connection_id, 0);
        assert_eq!(local.machines.active, LOCAL);
        assert_eq!(local.machines.inactive["remote"].connection_id, 7);
    }

    #[test]
    fn stale_reader_disconnect_cannot_disconnect_a_replacement_machine_link() {
        let (mut local, _) = super::super::tests::driver(120);
        let (mut remote, _) = super::super::tests::driver(120);
        local.machines.inactive.insert(
            "remote".into(),
            MachineEndpoint {
                shell: remote.state.shell.take().unwrap(),
                connection: remote.write_stream,
                commands: remote.endpoint_commands,
                connection_id: 9,
            },
        );
        local.background_disconnected(8);
        assert!(local.machines.inactive["remote"].connection.is_connected());
        local.background_disconnected(9);
        assert!(!local.machines.inactive["remote"].connection.is_connected());
        assert!(local.write_stream.is_connected());
    }

    #[test]
    fn remote_failure_does_not_end_local_client_or_replay_to_local_connection() {
        let (mut local, local_sent) = super::super::tests::driver(120);
        let (mut remote, _) = super::super::tests::driver(120);
        local.machines.inactive.insert(
            "remote".into(),
            MachineEndpoint {
                shell: remote.state.shell.take().unwrap(),
                connection: remote.write_stream,
                commands: remote.endpoint_commands,
                connection_id: 7,
            },
        );
        local.switch_machine("remote", RoomId(1));
        let writes = local_sent.lock().unwrap().len();
        assert!(!local
            .remote_failure(ClientError::ConnectionLost(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "down"
            )))
            .unwrap());
        assert!(!local.write_stream.is_connected());
        assert_eq!(local_sent.lock().unwrap().len(), writes);
        assert!(local.machines.inactive[LOCAL].connection.is_connected());
        assert!(local.switch_machine(LOCAL, RoomId(1)));
    }
}
