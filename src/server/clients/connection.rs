use std::collections::HashMap;
use std::path::PathBuf;

use crate::protocol::wire::{
    ClientKeyCode, ClientKeyKind, ClientMouseButton, ClientMouseKind, ClientPaneInputEvent,
};
use crate::server::clients::transport::ClientWriter;
use crate::server::rendering::stream::ClientRenderState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientConnectionMode {
    ClientShell,
}

pub(crate) type RenderTarget = (
    u64,
    (u16, u16),
    crate::protocol::kitty::HostCellSize,
    bool,
    ClientConnectionMode,
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClientShellInputTarget {
    Pane(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ClientShellPressId {
    PhysicalKey(u32),
    SemanticKey(ClientKeyCode),
    Mouse(ClientMouseButton),
}

fn client_shell_key_press_id(
    code: &ClientKeyCode,
    physical_key_id: Option<u32>,
) -> ClientShellPressId {
    physical_key_id.map_or_else(
        || ClientShellPressId::SemanticKey(code.clone()),
        ClientShellPressId::PhysicalKey,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClientShellHeldInput {
    pub(crate) target: ClientShellInputTarget,
    pub(crate) release: ClientPaneInputEvent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DeferredRender {
    #[default]
    None,
    Full,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClientShellLocation {
    pub(crate) focused_workspace_id: Option<String>,
    pub(crate) active_tab_ids: HashMap<String, String>,
}

pub(crate) struct ClientShellTopology {
    pub(crate) focused_workspace_id: Option<String>,
    pub(crate) fallback_workspace_id: Option<String>,
    pub(crate) active_tab_ids: HashMap<String, String>,
    pub(crate) tab_workspace_ids: HashMap<String, String>,
}

impl ClientShellLocation {
    pub(crate) fn from_snapshot(snapshot: &crate::protocol::wire::ClientShellSnapshot) -> Self {
        Self {
            focused_workspace_id: snapshot.focused_workspace_id.clone(),
            active_tab_ids: snapshot
                .workspaces
                .iter()
                .map(|workspace| {
                    (
                        workspace.workspace_id.clone(),
                        workspace.active_tab_id.clone(),
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn focused_tab_id(&self) -> Option<&str> {
        self.focused_workspace_id
            .as_deref()
            .and_then(|workspace_id| self.active_tab_ids.get(workspace_id))
            .map(String::as_str)
    }

    pub(crate) fn focus_workspace(&mut self, workspace_id: String) {
        self.focused_workspace_id = Some(workspace_id);
    }

    pub(crate) fn focus_tab(&mut self, workspace_id: String, tab_id: String) {
        self.focused_workspace_id = Some(workspace_id.clone());
        self.active_tab_ids.insert(workspace_id, tab_id);
    }

    pub(crate) fn reconcile(&mut self, topology: &ClientShellTopology) {
        self.active_tab_ids.retain(|workspace_id, tab_id| {
            topology.active_tab_ids.contains_key(workspace_id)
                && topology.tab_workspace_ids.get(tab_id) == Some(workspace_id)
        });
        for (workspace_id, tab_id) in &topology.active_tab_ids {
            self.active_tab_ids
                .entry(workspace_id.clone())
                .or_insert_with(|| tab_id.clone());
        }
        if self
            .focused_workspace_id
            .as_ref()
            .is_none_or(|workspace_id| !topology.active_tab_ids.contains_key(workspace_id))
        {
            self.focused_workspace_id = topology
                .focused_workspace_id
                .clone()
                .or_else(|| topology.fallback_workspace_id.clone());
        }
    }
}

/// A connected client tracked by the server.
pub(crate) struct ClientConnection {
    /// The client-owned shell connection mode.
    pub(crate) mode: ClientConnectionMode,
    /// The client's terminal size after clamping.
    pub(crate) terminal_size: (u16, u16),
    /// Pixel size of one client terminal cell.
    pub(crate) cell_size: crate::protocol::kitty::HostCellSize,
    /// Monotonic activity stamp used to choose the fallback foreground client.
    pub(crate) last_activity: u64,
    /// Render baseline for the negotiated client encoding.
    pub(crate) render_state: ClientRenderState,
    /// Image assets already included in the selected ClientShell scene.
    pub(crate) shell_graphics_delivery: crate::server::rendering::images::DeliveryCache,
    /// Passive eligibility for audited local Kitty regular-file graphics.
    pub(crate) direct_graphics: bool,
    /// Whether this frontend preserves exact SGR pixel reports.
    pub(crate) pixel_mouse: bool,
    /// Last host terminal default colors reported by this client.
    pub(crate) host_terminal_theme: crate::utils::theme::color::TerminalTheme,
    /// Last host light/dark appearance reported by this client.
    pub(crate) host_terminal_appearance: Option<crate::utils::theme::color::HostAppearance>,
    /// Whether appearance came from an explicit host color-scheme report.
    pub(crate) host_terminal_appearance_explicit: bool,
    /// Last reported focus state for this client's outer terminal.
    pub(crate) outer_terminal_focus: Option<bool>,
    /// Last focused-pane report-all demand sent to a client-owned shell.
    pub(crate) host_keyboard_report_all_active: Option<bool>,
    /// Whether an ordinary render was skipped because the render channel was full.
    pub(crate) render_pending: bool,
    /// Whether this connection receives pane surfaces and may affect presentation state.
    pub(crate) shell_surface_active: bool,
    /// Whether this shell wants host mouse capture without pane demand.
    pub(crate) shell_mouse_capture: bool,
    /// Last host mouse capture mode sent to this client.
    pub(crate) host_mouse_capture_active: Option<bool>,
    /// Last SGR pixel provenance mode sent to this client.
    pub(crate) host_sgr_pixels_active: Option<bool>,
    /// Presses forwarded by this shell that need release on abrupt teardown.
    shell_held_inputs: HashMap<ClientShellPressId, ClientShellHeldInput>,
    /// Temporary files staged from this client's local clipboard image pastes.
    pub(crate) staged_clipboard_files: Vec<PathBuf>,
    /// Connection-local workspace and tab projection for a client-owned shell.
    pub(crate) shell_location: Option<ClientShellLocation>,
    /// Last coherent shell replacement sent to this client.
    pub(crate) shell_snapshot: Option<crate::protocol::wire::ClientShellSnapshot>,
    /// Monotonic shell replacement revision for this connection.
    pub(crate) shell_projection_revision: u64,
    /// Whether this shell is waiting for one ordered endpoint command response.
    pub(crate) shell_endpoint_command_in_flight: bool,
    /// Surface projection epoch that owned the in-flight command. Deferred navigation may run
    /// only if this exact presentation lease is still active when its response arrives.
    pub(crate) shell_endpoint_command_surface_revision: Option<u64>,
    /// Whether this shell uses the endpoint-owned keymap rather than a client-owned keymap.
    pub(crate) shell_uses_endpoint_keybindings: bool,
    /// Channels for sending framed ServerMessage data to the client writer thread.
    pub(crate) writer: ClientWriter,
}

impl ClientConnection {
    #[cfg(test)]
    pub(crate) fn new(
        terminal_size: (u16, u16),
        cell_size: crate::protocol::kitty::HostCellSize,
        last_activity: u64,
        writer: ClientWriter,
    ) -> Self {
        Self::new_with_mode(
            ClientConnectionMode::ClientShell,
            terminal_size,
            cell_size,
            last_activity,
            writer,
        )
    }

    pub(crate) fn new_with_mode(
        mode: ClientConnectionMode,
        terminal_size: (u16, u16),
        cell_size: crate::protocol::kitty::HostCellSize,
        last_activity: u64,
        writer: ClientWriter,
    ) -> Self {
        Self {
            mode,
            terminal_size,
            cell_size,
            last_activity,
            render_state: ClientRenderState::default(),
            shell_graphics_delivery: crate::server::rendering::images::DeliveryCache::default(),
            direct_graphics: false,
            pixel_mouse: false,
            host_terminal_theme: crate::utils::theme::color::TerminalTheme::default(),
            host_terminal_appearance: None,
            host_terminal_appearance_explicit: false,
            outer_terminal_focus: None,
            host_keyboard_report_all_active: None,
            render_pending: false,
            shell_surface_active: true,
            shell_mouse_capture: false,
            host_mouse_capture_active: None,
            host_sgr_pixels_active: None,
            shell_held_inputs: HashMap::new(),
            staged_clipboard_files: Vec::new(),
            shell_location: None,
            shell_snapshot: None,
            shell_projection_revision: 0,
            shell_endpoint_command_in_flight: false,
            shell_endpoint_command_surface_revision: None,
            shell_uses_endpoint_keybindings: false,
            writer,
        }
    }

    pub(crate) fn request_repaint(&mut self) {
        self.render_state.request_repaint();
    }

    pub(crate) fn track_shell_input(
        &mut self,
        target: ClientShellInputTarget,
        events: &[ClientPaneInputEvent],
    ) {
        for event in events {
            match event {
                ClientPaneInputEvent::Key {
                    code,
                    modifiers,
                    kind: ClientKeyKind::Press,
                    shifted_codepoint,
                    tracks_release: true,
                    physical_key_id,
                    windows_record,
                    ..
                } => {
                    self.shell_held_inputs.insert(
                        client_shell_key_press_id(code, *physical_key_id),
                        ClientShellHeldInput {
                            target: target.clone(),
                            release: ClientPaneInputEvent::Key {
                                code: code.clone(),
                                modifiers: *modifiers,
                                kind: ClientKeyKind::Release,
                                repeat_count: 1,
                                shifted_codepoint: *shifted_codepoint,
                                generated_text: None,
                                tracks_release: true,
                                physical_key_id: *physical_key_id,
                                windows_record: *windows_record,
                            },
                        },
                    );
                }
                ClientPaneInputEvent::Key {
                    code,
                    kind: ClientKeyKind::Release,
                    physical_key_id,
                    ..
                } => {
                    self.shell_held_inputs
                        .remove(&client_shell_key_press_id(code, *physical_key_id));
                }
                ClientPaneInputEvent::Mouse {
                    kind: ClientMouseKind::Down(button),
                    position,
                    geometry,
                    modifiers,
                    ..
                }
                | ClientPaneInputEvent::Mouse {
                    kind: ClientMouseKind::Drag(button),
                    position,
                    geometry,
                    modifiers,
                    ..
                } => {
                    let id = ClientShellPressId::Mouse(*button);
                    if matches!(
                        event,
                        ClientPaneInputEvent::Mouse {
                            kind: ClientMouseKind::Down(_),
                            ..
                        }
                    ) || self.shell_held_inputs.contains_key(&id)
                    {
                        self.shell_held_inputs.insert(
                            id,
                            ClientShellHeldInput {
                                target: target.clone(),
                                release: ClientPaneInputEvent::Mouse {
                                    kind: ClientMouseKind::Up(*button),
                                    position: *position,
                                    geometry: *geometry,
                                    modifiers: *modifiers,
                                    lines: 1,
                                },
                            },
                        );
                    }
                }
                ClientPaneInputEvent::Mouse {
                    kind: ClientMouseKind::Up(button),
                    ..
                } => {
                    self.shell_held_inputs
                        .remove(&ClientShellPressId::Mouse(*button));
                }
                ClientPaneInputEvent::Key {
                    kind: ClientKeyKind::Press | ClientKeyKind::Repeat,
                    ..
                }
                | ClientPaneInputEvent::TextCommit(_)
                | ClientPaneInputEvent::Mouse { .. }
                | ClientPaneInputEvent::Paste(_) => {}
            }
        }
    }

    pub(crate) fn drain_shell_held_inputs(&mut self) -> Vec<ClientShellHeldInput> {
        self.shell_held_inputs
            .drain()
            .map(|(_, held)| held)
            .collect()
    }

    pub(crate) fn update_host_theme(
        &mut self,
        update: &crate::protocol::wire::ClientHostThemeUpdate,
    ) -> bool {
        let mut next_theme = self.host_terminal_theme;
        let mut changed = false;

        match update {
            crate::protocol::wire::ClientHostThemeUpdate::DefaultColor { kind, color } => {
                let kind = match kind {
                    crate::protocol::wire::ClientHostDefaultColorKind::Foreground => {
                        crate::utils::theme::color::DefaultColorKind::Foreground
                    }
                    crate::protocol::wire::ClientHostDefaultColorKind::Background => {
                        crate::utils::theme::color::DefaultColorKind::Background
                    }
                };
                let color = (*color).into();
                next_theme = next_theme.with_color(kind, color);
                if matches!(
                    kind,
                    crate::utils::theme::color::DefaultColorKind::Background
                ) && !self.host_terminal_appearance_explicit
                {
                    changed |= self.set_host_appearance(Some(color.inferred_appearance()), false);
                }
            }
            crate::protocol::wire::ClientHostThemeUpdate::PaletteColors(colors) => {
                for &(index, color) in colors {
                    next_theme = next_theme.with_palette_color(index, color.into());
                }
            }
            crate::protocol::wire::ClientHostThemeUpdate::Appearance(appearance) => {
                let appearance = match appearance {
                    crate::protocol::wire::ClientHostAppearance::Dark => {
                        crate::utils::theme::color::HostAppearance::Dark
                    }
                    crate::protocol::wire::ClientHostAppearance::Light => {
                        crate::utils::theme::color::HostAppearance::Light
                    }
                };
                changed |= self.set_host_appearance(Some(appearance), true);
            }
        }

        if next_theme != self.host_terminal_theme {
            self.host_terminal_theme = next_theme;
            changed = true;
        }
        changed
    }

    fn set_host_appearance(
        &mut self,
        appearance: Option<crate::utils::theme::color::HostAppearance>,
        explicit: bool,
    ) -> bool {
        if self.host_terminal_appearance_explicit && !explicit {
            return false;
        }
        if self.host_terminal_appearance == appearance
            && self.host_terminal_appearance_explicit == explicit
        {
            return false;
        }
        self.host_terminal_appearance = appearance;
        self.host_terminal_appearance_explicit = explicit;
        true
    }

    pub(crate) fn deferred_render(&self) -> DeferredRender {
        if self.render_pending {
            DeferredRender::Full
        } else {
            DeferredRender::None
        }
    }

    pub(crate) fn clear_deferred_render(&mut self) {
        self.render_pending = false;
    }

    pub(crate) fn defer_full_render(&mut self) {
        self.render_pending = true;
    }

    pub(crate) fn take_deferred_render(&mut self) -> DeferredRender {
        let deferred = self.deferred_render();
        self.clear_deferred_render();
        deferred
    }

    pub(crate) fn is_shell_client(&self) -> bool {
        matches!(self.mode, ClientConnectionMode::ClientShell)
    }

    pub(crate) fn is_active_shell_client(&self) -> bool {
        self.is_shell_client() && self.shell_surface_active
    }
}

pub(crate) fn latest_shell_client(clients: &HashMap<u64, ClientConnection>) -> Option<u64> {
    clients
        .iter()
        .filter(|(_, client)| client.is_active_shell_client())
        .max_by_key(|(_, client)| client.last_activity)
        .map(|(&client_id, _)| client_id)
}

pub(crate) fn render_targets(
    clients: &HashMap<u64, ClientConnection>,
    foreground_client_id: Option<u64>,
) -> Vec<RenderTarget> {
    let mut targets: Vec<RenderTarget> = clients
        .iter()
        .map(|(&client_id, client)| {
            (
                client_id,
                client.terminal_size,
                client.cell_size,
                foreground_client_id == Some(client_id),
                client.mode.clone(),
            )
        })
        .collect();

    targets.sort_by_key(|(client_id, _, _, is_foreground, _)| (*is_foreground, *client_id));
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_client() -> ClientConnection {
        ClientConnection::new(
            (80, 24),
            crate::protocol::kitty::HostCellSize::default(),
            1,
            unread_test_writer(),
        )
    }

    /// A writer for a client whose output the test never reads; its channels
    /// stay open so sends succeed as they do for a live client.
    fn unread_test_writer() -> crate::server::clients::transport::ClientWriter {
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let (render_tx, render_rx) = std::sync::mpsc::sync_channel(1);
        std::mem::forget((control_rx, render_rx));
        crate::server::clients::transport::ClientWriter::test_channel(control_tx, render_tx)
    }

    #[test]
    fn semantic_text_press_does_not_create_a_server_release_lease() {
        let mut client = shell_client();
        client.track_shell_input(
            ClientShellInputTarget::Pane("w1:p1".into()),
            &[ClientPaneInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Char('x'),
                modifiers: 0,
                kind: ClientKeyKind::Press,
                repeat_count: 1,
                shifted_codepoint: None,
                generated_text: Some("x".into()),
                tracks_release: false,
                physical_key_id: None,
                windows_record: None,
            }],
        );

        assert!(client.drain_shell_held_inputs().is_empty());
    }

    #[test]
    fn physical_keys_with_the_same_semantic_code_keep_distinct_release_leases() {
        let mut client = shell_client();
        let windows_record = crate::protocol::keys::WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code: 0x6c,
            virtual_scan_code: 0x1c,
            unicode: 13,
            control_key_state: 0,
        };
        let key = |kind, physical_key_id| ClientPaneInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Enter,
            modifiers: 0,
            kind,
            repeat_count: 1,
            shifted_codepoint: None,
            generated_text: None,
            tracks_release: true,
            physical_key_id: Some(physical_key_id),
            windows_record: (physical_key_id == 108).then_some(windows_record),
        };
        client.track_shell_input(
            ClientShellInputTarget::Pane("w1:p1".into()),
            &[
                key(ClientKeyKind::Press, 13),
                key(ClientKeyKind::Press, 108),
                key(ClientKeyKind::Release, 13),
            ],
        );

        let held = client.drain_shell_held_inputs();
        assert_eq!(held.len(), 1);
        assert!(matches!(
            &held[0].release,
            ClientPaneInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Enter,
                kind: ClientKeyKind::Release,
                physical_key_id: Some(108),
                windows_record: Some(record),
                ..
            } if *record == windows_record
        ));
    }
}
