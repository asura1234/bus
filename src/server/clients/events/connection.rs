use super::ServerEvent;
use crate::protocol::{self, ServerMessage};
use crate::server::clients::connection::{
    ClientConnection, ClientConnectionMode, ClientShellInputTarget, DeferredRender,
};
use crate::server::clients::input::apply_client_pane_input_events;
use crate::server::main_loop::HeadlessServer;
use crate::server::rendering::snapshot::snapshot as client_shell_snapshot;
use std::sync::atomic::Ordering;
use tracing::{info, warn};

impl HeadlessServer {
    pub(in crate::server) fn remove_client(&mut self, client_id: u64) -> bool {
        let disconnected_focus = self
            .clients
            .get(&client_id)
            .filter(|client| {
                client.is_active_shell_client() && client.outer_terminal_focus == Some(true)
            })
            .and_then(|_| self.shell_focus_target(client_id));
        let should_release_focus = disconnected_focus.as_ref().is_some_and(|target| {
            !self.clients.iter().any(|(&other_id, client)| {
                other_id != client_id
                    && client.is_active_shell_client()
                    && client.outer_terminal_focus == Some(true)
                    && self.shell_tab_id_for_client(other_id).as_deref()
                        == Some(target.tab_id.as_str())
            })
        });
        let was_foreground = self.foreground_client_id == Some(client_id);
        let removed = self.clients.remove(&client_id);
        self.tab_geometry_controllers
            .retain(|_, controller_id| *controller_id != client_id);
        if let Some(mut removed) = removed {
            let held_inputs = removed.drain_shell_held_inputs();
            self.release_client_shell_inputs(client_id, held_inputs);
            crate::server::clients::clipboard_images::remove_files(removed.staged_clipboard_files);
        }
        if should_release_focus {
            if let Some(target) = disconnected_focus.as_ref() {
                self.send_shell_focus_target(target, crate::ghostty::FocusEvent::Lost);
            }
        }
        if was_foreground {
            self.promote_latest_remaining_client()
        } else {
            false
        }
    }

    pub(in crate::server) fn release_client_shell_inputs(
        &mut self,
        client_id: u64,
        held_inputs: Vec<crate::server::clients::connection::ClientShellHeldInput>,
    ) {
        for held in held_inputs {
            let result = match held.target {
                ClientShellInputTarget::Pane(pane_id) => {
                    let Some((workspace_index, runtime_pane_id)) = self.app.parse_pane_id(&pane_id)
                    else {
                        continue;
                    };
                    let Some(runtime) = self.app.state.runtime_for_pane_in_workspace(
                        &self.app.terminal_runtimes,
                        workspace_index,
                        runtime_pane_id,
                    ) else {
                        continue;
                    };
                    apply_client_pane_input_events(runtime, &[held.release])
                }
            };
            if let Err(err) = result {
                warn!(target: "bus::server::main_loop", client_id, err = %err, "client shell teardown release failed");
            }
        }
    }

    pub(in crate::server) fn remove_client_and_resize_if_needed(&mut self, client_id: u64) {
        self.remove_client(client_id);
        self.resize_tabs_for_only_shell_client(true);
    }

    /// Drains server events from the dedicated channel.
    pub(in crate::server) fn drain_server_events(&mut self) -> bool {
        let mut changed = false;
        while !self.should_quit.load(Ordering::Acquire) {
            let Ok(ev) = self.server_event_rx.try_recv() else {
                break;
            };
            changed |= self.handle_server_event(ev);
        }
        changed
    }

    pub(in crate::server) async fn reject_late_client_connections(&mut self) {
        self.server_event_rx.close();
        while let Some(event) = self.server_event_rx.recv().await {
            if let ServerEvent::ClientShellConnected { writer, .. } = event {
                if let Ok(message) = Self::frame_server_message(&ServerMessage::ServerShutdown {
                    reason: Some("server is shutting down".to_owned()),
                }) {
                    let _ = writer.control.send(message);
                }
            }
        }
    }
}

impl HeadlessServer {
    pub(super) fn handle_client_connection_event(&mut self, ev: ServerEvent) -> bool {
        match ev {
            ServerEvent::ClientShellConnected {
                client_id,
                surface_cols,
                surface_rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse,
                direct_graphics,
                endpoint_keybindings,
                mouse_capture,
                surface_active,
                writer,
            } => {
                info!(target: "bus::server::main_loop",
                    client_id,
                    cols = surface_cols,
                    rows = surface_rows,
                    cell_width_px,
                    cell_height_px,
                    surface_active,
                    render_encoding = ?protocol::RenderEncoding::SemanticFrame,
                    "client connected"
                );
                self.app.ensure_default_workspace();
                let last_activity = self.allocate_activity_stamp();
                let observed = crate::kitty_graphics::HostCellSize {
                    width_px: cell_width_px,
                    height_px: cell_height_px,
                };
                let mut connection = ClientConnection::new_with_mode(
                    ClientConnectionMode::ClientShell,
                    (surface_cols, surface_rows),
                    observed,
                    last_activity,
                    writer,
                );
                connection.pixel_mouse = pixel_mouse && observed.is_known();
                connection.direct_graphics = direct_graphics;
                connection.shell_uses_endpoint_keybindings = endpoint_keybindings;
                connection.shell_mouse_capture = mouse_capture;
                connection.shell_surface_active = surface_active;
                connection.shell_projection_revision = 1;
                let config_diagnostic = self.server_config_diagnostic.as_deref();
                let seed_snapshot = client_shell_snapshot(
                    &self.app,
                    &self.client_shell_boot_id,
                    connection.shell_projection_revision,
                    config_diagnostic,
                    None,
                );
                let location =
                    crate::server::clients::connection::ClientShellLocation::from_snapshot(
                        &seed_snapshot,
                    );
                let snapshot_message = match crate::protocol::endpoint::snapshot_message(
                    &seed_snapshot,
                ) {
                    Ok(message) => message,
                    Err(err) => {
                        warn!(target: "bus::server::main_loop", client_id, err = %err, "failed to encode endpoint snapshot");
                        return false;
                    }
                };
                connection.shell_location = Some(location);
                connection.shell_snapshot = Some(seed_snapshot);
                self.clients.insert(client_id, connection);
                self.send_to_client(client_id, snapshot_message);
                if surface_active {
                    self.foreground_client_id = Some(client_id);
                }
                self.sync_foreground_client_state();
                self.claim_unowned_shell_tab_geometry(client_id, true);
                true
            }
            ServerEvent::ClientDetach { client_id } => {
                info!(target: "bus::server::main_loop", client_id, "client detached");
                self.remove_client_and_resize_if_needed(client_id);
                true
            }
            ServerEvent::ClientDisconnected { client_id } => {
                info!(target: "bus::server::main_loop", client_id, "client disconnected");
                self.remove_client_and_resize_if_needed(client_id);
                true
            }
            ServerEvent::ClientWriterDrained { client_id } => {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return false;
                };
                client.take_deferred_render() != DeferredRender::None
            }
            ServerEvent::QuitSignal => {
                // The quit check at the top of the loop handles this.
                // No render needed — the next iteration will initiate shutdown.
                false
            }
            _ => unreachable!("connection handler only receives lifecycle events"),
        }
    }
}
