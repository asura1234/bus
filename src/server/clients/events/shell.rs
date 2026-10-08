use super::ServerEvent;
use crate::protocol::{self, ServerMessage};
use crate::server::clients::connection::{
    ClientConnection, ClientConnectionMode, ClientShellInputTarget,
};
use crate::server::clients::input::apply_client_pane_input_events;
use crate::server::main_loop::HeadlessServer;
use tracing::{debug, info, warn};

impl HeadlessServer {
    pub(super) fn handle_client_shell_event(&mut self, ev: ServerEvent) -> bool {
        match ev {
            ServerEvent::ClientPasteRejected {
                client_id,
                size,
                max,
            } => {
                let detail = format!("Input message is {size} bytes; Herdr's limit is {max} bytes");
                let message = ServerMessage::ClientShellError {
                    message: format!("Paste rejected: {detail}"),
                };
                self.send_to_client(client_id, message);
                false
            }
            ServerEvent::ClientClipboardImage {
                client_id,
                target,
                extension,
                data,
            } => {
                debug!(target: "bus::server::main_loop",
                    client_id,
                    len = data.len(),
                    extension = %extension,
                    "client clipboard image received"
                );
                if !self.client_clipboard_image_target_is_valid(client_id, &target) {
                    return false;
                }
                match self.stage_client_clipboard_image(client_id, &extension, &data) {
                    Ok(staged) => {
                        let routed = self.paste_client_clipboard_image_path(
                            client_id,
                            target,
                            staged.paste_text,
                        );
                        if routed {
                            if let Some(client) = self.clients.get_mut(&client_id) {
                                client.staged_clipboard_files.push(staged.path);
                            } else {
                                crate::server::clients::clipboard_images::remove_files(vec![
                                    staged.path,
                                ]);
                                return false;
                            }
                        } else {
                            crate::server::clients::clipboard_images::remove_files(vec![
                                staged.path,
                            ]);
                        }
                        routed
                    }
                    Err(err) => {
                        warn!(target: "bus::server::main_loop", client_id, err = %err, "failed to stage client clipboard image");
                        true
                    }
                }
            }
            ServerEvent::ClientShellResize {
                client_id,
                surface_cols,
                surface_rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse,
            } => {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return false;
                };
                if !matches!(client.mode, ClientConnectionMode::ClientShell) {
                    return false;
                }
                client.terminal_size = (surface_cols, surface_rows);
                let observed = crate::kitty_graphics::HostCellSize {
                    width_px: cell_width_px,
                    height_px: cell_height_px,
                };
                if observed.is_known() {
                    client.cell_size = observed;
                }
                client.pixel_mouse = pixel_mouse && observed.is_known();
                if !client.shell_surface_active {
                    return false;
                }
                client.request_repaint();
                self.promote_client_to_foreground(client_id);
                self.resize_shell_tab_if_controller(client_id, true);
                true
            }
            ServerEvent::ClientShellHostTheme { client_id, update } => {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return false;
                };
                if !matches!(client.mode, ClientConnectionMode::ClientShell) {
                    return false;
                }
                if !client.update_host_theme(&update) {
                    return false;
                }
                if !client.shell_surface_active || self.foreground_client_id != Some(client_id) {
                    return false;
                }
                let mut changed = self.app.set_host_terminal_appearance_state(
                    client.host_terminal_appearance,
                    client.host_terminal_appearance_explicit,
                );
                changed |= self.app.set_host_terminal_theme(client.host_terminal_theme);
                if changed {
                    self.resize_shared_runtime_to_effective_size_before_input();
                }
                changed
            }
            ServerEvent::ClientShellFocus { client_id, focused } => {
                let Some(client) = self.clients.get(&client_id) else {
                    return false;
                };
                if !client.is_active_shell_client() || client.outer_terminal_focus == Some(focused)
                {
                    return false;
                }
                let tab_id = self.shell_tab_id_for_client(client_id);
                let another_focused_viewer = self.clients.iter().any(|(&other_id, client)| {
                    other_id != client_id
                        && client.is_active_shell_client()
                        && client.outer_terminal_focus == Some(true)
                        && self.shell_tab_id_for_client(other_id) == tab_id
                });
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.outer_terminal_focus = Some(focused);
                }
                if focused {
                    self.promote_client_to_foreground(client_id);
                    self.claim_shell_tab_geometry(client_id, false);
                    if !another_focused_viewer {
                        if let Some(target) = self.shell_focus_target(client_id) {
                            self.send_shell_focus_target(
                                &target,
                                crate::ghostty::FocusEvent::Gained,
                            );
                        }
                    }
                    true
                } else {
                    if self.foreground_client_id == Some(client_id) {
                        self.app.state.outer_terminal_focus = Some(false);
                    }
                    if !another_focused_viewer {
                        if let Some(target) = self.shell_focus_target(client_id) {
                            self.send_shell_focus_target(&target, crate::ghostty::FocusEvent::Lost);
                        }
                    }
                    true
                }
            }
            ServerEvent::ClientShellMouseCapture { client_id, enabled } => {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return false;
                };
                if !matches!(client.mode, ClientConnectionMode::ClientShell)
                    || client.shell_mouse_capture == enabled
                {
                    return false;
                }
                client.shell_mouse_capture = enabled;
                client.host_mouse_capture_active = None;
                true
            }
            ServerEvent::ClientShellPresentationSync { client_id, token } => {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return false;
                };
                if !client.is_active_shell_client() {
                    return false;
                }
                client.host_mouse_capture_active = None;
                client.host_sgr_pixels_active = None;
                client.host_keyboard_report_all_active = None;
                self.sent_window_title = None;
                self.stream_host_mouse_capture_mode();
                self.stream_direct_terminal_keyboard_mode();
                self.sync_window_title();
                self.send_to_client(
                    client_id,
                    ServerMessage::EndpointControl {
                        kind: crate::protocol::endpoint::PRESENTATION_EFFECTS_READY_KIND.into(),
                        data: token,
                    },
                )
            }
            ServerEvent::ClientShellPaneInput {
                client_id,
                pane_id,
                events,
            } => {
                if !self
                    .clients
                    .get(&client_id)
                    .is_some_and(ClientConnection::is_active_shell_client)
                {
                    return false;
                }
                let pixel_mouse = self.clients.get(&client_id).is_some_and(|client| {
                    client.pixel_mouse && client.host_sgr_pixels_active == Some(true)
                });
                let mut events = events;
                let Some((workspace_index, runtime_pane_id)) = self.app.parse_pane_id(&pane_id)
                else {
                    return false;
                };
                let Some(runtime) = self.app.state.runtime_for_pane_in_workspace(
                    &self.app.terminal_runtimes,
                    workspace_index,
                    runtime_pane_id,
                ) else {
                    return false;
                };
                crate::server::clients::input::downgrade_ineligible_pixel_mouse(
                    &mut events,
                    pixel_mouse,
                    runtime.current_size(),
                    runtime.pixel_size(),
                );
                if !self.shell_client_views_pane(client_id, workspace_index, runtime_pane_id) {
                    let Some(runtime) = self.app.state.runtime_for_pane_in_workspace(
                        &self.app.terminal_runtimes,
                        workspace_index,
                        runtime_pane_id,
                    ) else {
                        return false;
                    };
                    let releases = events
                        .into_iter()
                        .filter(client_pane_input_releases_press)
                        .collect::<Vec<_>>();
                    if releases.is_empty() {
                        return false;
                    }
                    if let Some(client) = self.clients.get_mut(&client_id) {
                        client.track_shell_input(
                            ClientShellInputTarget::Pane(pane_id.clone()),
                            &releases,
                        );
                    }
                    let scroll_before = runtime.scroll_metrics();
                    if let Err(err) = apply_client_pane_input_events(runtime, &releases) {
                        warn!(target: "bus::server::main_loop", client_id, pane_id, err = %err, "targeted client shell release failed");
                    }
                    return runtime.scroll_metrics() != scroll_before;
                }
                let interaction = client_pane_input_has_interaction(&events);
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client
                        .track_shell_input(ClientShellInputTarget::Pane(pane_id.clone()), &events);
                }
                let foreground_changed =
                    interaction && self.promote_client_to_foreground(client_id);
                let geometry_changed =
                    interaction && self.claim_shell_tab_geometry(client_id, false);
                let Some(runtime) = self.app.state.runtime_for_pane_in_workspace(
                    &self.app.terminal_runtimes,
                    workspace_index,
                    runtime_pane_id,
                ) else {
                    return foreground_changed | geometry_changed;
                };
                let scroll_before = runtime.scroll_metrics();
                if let Err(err) = apply_client_pane_input_events(runtime, &events) {
                    warn!(target: "bus::server::main_loop", client_id, pane_id, err = %err, "targeted client shell input failed");
                }
                foreground_changed | geometry_changed || runtime.scroll_metrics() != scroll_before
            }
            ServerEvent::ClientShellEndpointRequestError {
                client_id,
                boot_id,
                request_id,
                code,
                message,
            } => {
                let Some(client) = self.clients.get(&client_id) else {
                    return false;
                };
                if !matches!(client.mode, ClientConnectionMode::ClientShell) {
                    self.remove_client_and_resize_if_needed(client_id);
                    return true;
                }
                let message = crate::server::clients::requests::error_message(
                    boot_id, request_id, code, message,
                );
                self.send_to_client(client_id, message);
                false
            }
            ServerEvent::ClientShellEndpointRequest {
                client_id,
                boot_id,
                request,
            } => self.handle_client_shell_endpoint_request(client_id, boot_id, *request),
            ServerEvent::ClientShellEndpointResponseChunkReady {
                client_id,
                boot_id,
                request_id,
                final_chunk,
                data,
            } => {
                let command_in_flight = self.clients.get(&client_id).is_some_and(|client| {
                    matches!(client.mode, ClientConnectionMode::ClientShell)
                        && client.shell_endpoint_command_in_flight
                        && boot_id == self.client_shell_boot_id
                });
                if !command_in_flight {
                    return false;
                }
                if final_chunk {
                    if let Some(client) = self.clients.get_mut(&client_id) {
                        client.shell_endpoint_command_in_flight = false;
                        client.shell_endpoint_command_surface_revision = None;
                    }
                }
                self.send_to_client(
                    client_id,
                    ServerMessage::ClientShellEndpointResponseChunk {
                        boot_id,
                        request_id,
                        final_chunk,
                        data,
                    },
                );
                false
            }
            _ => unreachable!("shell handler only receives shell events"),
        }
    }
}

fn client_pane_input_releases_press(event: &protocol::ClientPaneInputEvent) -> bool {
    matches!(
        event,
        protocol::ClientPaneInputEvent::Key {
            kind: protocol::ClientKeyKind::Release,
            ..
        } | protocol::ClientPaneInputEvent::Mouse {
            kind: protocol::ClientMouseKind::Up(_),
            ..
        }
    )
}

fn client_pane_input_has_interaction(events: &[protocol::ClientPaneInputEvent]) -> bool {
    events
        .iter()
        .any(|event| !client_pane_input_releases_press(event))
}

impl HeadlessServer {
    pub(in crate::server) fn client_clipboard_image_target_is_valid(
        &self,
        client_id: u64,
        target: &protocol::ClientClipboardImageTarget,
    ) -> bool {
        match target {
            protocol::ClientClipboardImageTarget::Pane(pane_id) => {
                self.clients
                    .get(&client_id)
                    .is_some_and(ClientConnection::is_active_shell_client)
                    && self.app.parse_pane_id(pane_id).is_some()
            }
        }
    }

    pub(in crate::server) fn stage_client_clipboard_image(
        &self,
        client_id: u64,
        extension: &str,
        data: &[u8],
    ) -> std::io::Result<crate::server::clients::clipboard_images::StagedClipboardImage> {
        let staged = crate::server::clients::clipboard_images::stage(client_id, extension, data)?;
        info!(target: "bus::server::main_loop", client_id, bytes = data.len(), path = %staged.paste_text, "staged client clipboard image");
        Ok(staged)
    }

    pub(in crate::server) fn paste_client_clipboard_image_path(
        &mut self,
        client_id: u64,
        target: protocol::ClientClipboardImageTarget,
        path: String,
    ) -> bool {
        match target {
            protocol::ClientClipboardImageTarget::Pane(pane_id) => {
                if !self
                    .clients
                    .get(&client_id)
                    .is_some_and(ClientConnection::is_active_shell_client)
                {
                    return false;
                }
                let Some((workspace_index, runtime_pane_id)) = self.app.parse_pane_id(&pane_id)
                else {
                    return false;
                };
                if !self.shell_client_views_pane(client_id, workspace_index, runtime_pane_id) {
                    return false;
                }
                let foreground_changed = self.promote_client_to_foreground(client_id);
                let geometry_changed = self.claim_shell_tab_geometry(client_id, false);
                let Some(runtime) = self.app.state.runtime_for_pane_in_workspace(
                    &self.app.terminal_runtimes,
                    workspace_index,
                    runtime_pane_id,
                ) else {
                    return foreground_changed | geometry_changed;
                };
                if let Err(err) = apply_client_pane_input_events(
                    runtime,
                    &[protocol::ClientPaneInputEvent::Paste(path)],
                ) {
                    warn!(target: "bus::server::main_loop", client_id, pane_id, err = %err, "client shell clipboard image paste failed");
                }
                true
            }
        }
    }
}
