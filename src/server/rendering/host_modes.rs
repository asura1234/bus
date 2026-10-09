use crate::protocol::wire::ServerMessage;
use crate::server::main_loop::HeadlessServer;
use tracing::{debug, warn};

impl HeadlessServer {
    fn shell_focused_runtime(
        &self,
        client_id: u64,
    ) -> Option<(&crate::terminal::TerminalRuntime, crate::utils::ids::PaneId)> {
        let target = self.shell_target_for_client(client_id)?;
        let tab = self
            .app
            .state
            .workspaces
            .get(target.workspace_index)?
            .tabs
            .get(target.tab_index)?;
        let pane_id = tab.layout.focused();
        self.app
            .state
            .runtime_for_pane_in_workspace(
                &self.app.terminal_runtimes,
                target.workspace_index,
                pane_id,
            )
            .map(|runtime| (runtime, pane_id))
    }

    pub(in crate::server) fn stream_host_mouse_capture_mode(&mut self) {
        let requested = self
            .clients
            .iter()
            .map(|(&client_id, client)| {
                let focused = client
                    .shell_surface_active
                    .then(|| self.shell_focused_runtime(client_id))
                    .flatten();
                let child_requests_mouse =
                    focused.is_some_and(|(runtime, _)| runtime.mouse_reporting_enabled());
                (
                    client_id,
                    client.shell_surface_active
                        && (client.shell_mouse_capture || child_requests_mouse),
                    false,
                )
            })
            .collect::<Vec<_>>();

        let mut broken_clients = Vec::new();
        for (client_id, enabled, sgr_pixels) in requested {
            let Some(client) = self.clients.get_mut(&client_id) else {
                continue;
            };
            if client.host_mouse_capture_active == Some(enabled)
                && client.host_sgr_pixels_active == Some(sgr_pixels)
            {
                continue;
            }
            let writer = &client.writer;
            let serialized = match Self::frame_server_message(&ServerMessage::MouseCapture {
                enabled,
                sgr_pixels,
            }) {
                Ok(framed) => framed,
                Err(err) => {
                    warn!(target: "bus::server::rendering::full", err = %err, "failed to serialize mouse capture mode for client");
                    continue;
                }
            };
            if writer.control.send(serialized).is_err() {
                debug!(target: "bus::server::rendering::full",
                    client_id,
                    "client writer channel closed during mouse capture update"
                );
                broken_clients.push(client_id);
                continue;
            }
            client.host_mouse_capture_active = Some(enabled);
            client.host_sgr_pixels_active = Some(sgr_pixels);
        }

        for client_id in broken_clients {
            self.remove_client_and_resize_if_needed(client_id);
        }
    }

    pub(in crate::server) fn stream_direct_terminal_keyboard_mode(&mut self) {
        let shell_modes = self
            .clients
            .iter()
            .filter(|(_, client)| client.is_shell_client())
            .map(|(&client_id, client)| {
                let report_all = client.shell_surface_active
                    && self
                        .shell_focused_runtime(client_id)
                        .is_some_and(|(runtime, _)| {
                            let protocol = runtime.keyboard_protocol();
                            protocol.reports_all_keys()
                                || (protocol.reports_event_types()
                                    && runtime.modify_other_keys_level() > 0)
                        });
                (client_id, report_all)
            })
            .collect::<Vec<_>>();
        let mut broken_clients = Vec::new();
        for (client_id, report_all) in shell_modes {
            let Some(client) = self.clients.get_mut(&client_id) else {
                continue;
            };
            if client.host_keyboard_report_all_active == Some(report_all) {
                continue;
            }
            let writer = &client.writer;
            let serialized = match Self::frame_server_message(
                &ServerMessage::ClientShellKeyboardReportAll {
                    enabled: report_all,
                },
            ) {
                Ok(serialized) => serialized,
                Err(err) => {
                    warn!(target: "bus::server::rendering::full", err = %err, "failed to serialize client shell keyboard report-all mode");
                    continue;
                }
            };
            if writer.control.send(serialized).is_err() {
                broken_clients.push(client_id);
                continue;
            }
            client.host_keyboard_report_all_active = Some(report_all);
        }

        for client_id in broken_clients {
            self.remove_client_and_resize_if_needed(client_id);
        }
    }
}
