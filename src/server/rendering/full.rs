use std::collections::HashSet;

use ratatui::layout::Rect;
use tracing::{debug, warn};

use crate::protocol::wire::{self as protocol, MAX_GRAPHICS_FRAME_SIZE};
use crate::server::clients::connection::{render_targets, ClientConnection};
use crate::server::main_loop::HeadlessServer;
use crate::server::rendering::snapshot::{
    render_pane_surface as render_client_shell_pane_surface, snapshot as client_shell_snapshot,
};

impl HeadlessServer {
    pub(in crate::server) fn has_pending_presentation_work(&self, needs_full_render: bool) -> bool {
        needs_full_render || self.app.render_dirty.has_immediate_work()
    }

    pub(in crate::server) fn sync_immediate_pty_sources(&self) {
        let mut pane_ids = HashSet::new();
        for (&client_id, client) in &self.clients {
            if !client.is_active_shell_client() {
                continue;
            }
            let Some(target) = self.shell_target_for_client(client_id) else {
                continue;
            };
            let Some(tab) = self
                .app
                .state
                .workspaces
                .get(target.workspace_index)
                .and_then(|workspace| workspace.tabs.get(target.tab_index))
            else {
                continue;
            };
            if tab.zoomed {
                pane_ids.insert(tab.layout.focused());
            } else {
                pane_ids.extend(tab.layout.pane_ids());
            }
        }
        self.app.render_dirty.set_immediate_pty_sources(pane_ids);
    }

    pub(in crate::server) fn pty_sources_visible_to_any_render_target(
        &self,
        sources: &HashSet<crate::utils::ids::PaneId>,
    ) -> bool {
        self.clients
            .values()
            .any(ClientConnection::is_active_shell_client)
            && sources.iter().copied().any(|pane_id| {
                self.terminal_id_for_pane(pane_id).is_none()
                    || self.any_shell_surface_contains_pane(pane_id)
            })
    }

    fn terminal_id_for_pane(
        &self,
        pane_id: crate::utils::ids::PaneId,
    ) -> Option<&crate::utils::ids::TerminalId> {
        self.app
            .find_pane(pane_id)
            .map(|(_, pane)| &pane.attached_terminal_id)
    }

    fn any_shell_surface_contains_pane(&self, pane_id: crate::utils::ids::PaneId) -> bool {
        self.clients.iter().any(|(&client_id, client)| {
            if !client.is_active_shell_client() {
                return false;
            }
            let Some(target) = self.shell_target_for_client(client_id) else {
                return false;
            };
            let Some(tab) = self
                .app
                .state
                .workspaces
                .get(target.workspace_index)
                .and_then(|workspace| workspace.tabs.get(target.tab_index))
            else {
                return false;
            };
            tab.panes.contains_key(&pane_id) && (!tab.zoomed || tab.layout.focused() == pane_id)
        })
    }

    pub(in crate::server) fn render_and_stream(&mut self) {
        let full_started = crate::utils::render::prof::timer();
        let render_targets = render_targets(&self.clients, self.foreground_client_id);

        if render_targets.is_empty() {
            let resize_panes = self.update_headless_render_geometry();
            self.app.full_redraw_pending = false;
            crate::utils::render::prof::duration_since("full_render.total", full_started);
            let (cols, rows) = self.effective_size;
            debug!(
                cols,
                rows, resize_panes, "updated geometry with no attached clients"
            );
            return;
        }

        let mut broken_clients: Vec<u64> = Vec::new();
        for (client_id, (cols, rows), cell_size, _is_foreground, _mode) in render_targets {
            if self
                .render_client_pane_surface(client_id, Rect::new(0, 0, cols, rows), cell_size)
                .is_err()
            {
                broken_clients.push(client_id);
            }
        }

        if !broken_clients.is_empty() {
            for client_id in broken_clients {
                self.remove_client_and_resize_if_needed(client_id);
            }
        }

        let (cols, rows) = self.effective_size;
        // Full-frame recovery is tracked per connection. A slow client must not
        // keep responsive peers on the global full-render path while it waits
        // for its render slot to drain.
        self.app.full_redraw_pending = false;
        crate::utils::render::prof::duration_since("full_render.total", full_started);
        debug!(cols, rows, foreground_client_id = ?self.foreground_client_id, "rendered virtual frame(s)");
    }
    fn update_headless_render_geometry(&mut self) -> bool {
        let (cols, rows) = self.effective_size;
        let area = Rect::new(0, 0, cols, rows);
        let resize_panes = self.app.state.view.pane_infos.is_empty();
        if resize_panes {
            crate::server::rendering::surface::compute_view_with_runtime_registry(
                &mut self.app.state,
                &self.app.terminal_runtimes,
                area,
            );
        } else {
            crate::server::rendering::surface::compute_view_without_resizing_panes(
                &mut self.app.state,
                &self.app.terminal_runtimes,
                area,
            );
        }
        resize_panes
    }

    fn sync_client_shell_snapshot(&mut self, client_id: u64) -> Result<bool, ()> {
        let location = self
            .clients
            .get(&client_id)
            .and_then(|client| client.shell_location.clone());
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Ok(false);
        };
        let mut candidate = client_shell_snapshot(
            &self.app,
            &self.client_shell_boot_id,
            client.shell_projection_revision,
            None,
            location.as_ref(),
        );
        candidate.config_diagnostic = self.server_config_diagnostic.clone();
        candidate.revision = client.shell_projection_revision;
        if client.shell_snapshot.as_ref() != Some(&candidate) {
            client.shell_projection_revision = client.shell_projection_revision.saturating_add(1);
            candidate.revision = client.shell_projection_revision;
            let message =
                crate::protocol::wire::handshake::snapshot_message(&candidate).map_err(|err| {
                    warn!(client_id, err = %err, "failed to encode endpoint snapshot");
                })?;
            let framed = Self::frame_server_message(&message).map_err(|err| {
                warn!(client_id, err = %err, "failed to frame endpoint snapshot");
            })?;
            client.writer.control.send(framed).map_err(|_| ())?;
            client.shell_snapshot = Some(candidate);
        }
        if !client.shell_surface_active {
            client.clear_deferred_render();
            return Ok(false);
        }
        Ok(true)
    }

    fn render_client_pane_surface(
        &mut self,
        client_id: u64,
        area: Rect,
        cell_size: crate::protocol::kitty::HostCellSize,
    ) -> Result<(), ()> {
        let shell_target = self.shell_target_for_client(client_id);
        if !self.sync_client_shell_snapshot(client_id)? {
            return Ok(());
        }
        let shell_graphics_delivery = self
            .clients
            .get(&client_id)
            .map(|client| client.shell_graphics_delivery.clone())
            .unwrap_or_default();
        let render_started = crate::utils::render::prof::timer();
        let crate::server::rendering::snapshot::RenderedPaneSurface {
            frame,
            panes,
            splits,
            graphics,
            graphics_delivery: delivery,
        } = {
            let render_cell_size = if cell_size.is_known() {
                cell_size
            } else {
                crate::protocol::kitty::HostCellSize::default()
            };
            let rendered = render_client_shell_pane_surface(
                &mut self.app,
                shell_target,
                area,
                false,
                render_cell_size,
                &shell_graphics_delivery,
            );
            crate::utils::render::prof::duration_since(
                "full_render.render_tab_surface_virtual",
                render_started,
            );
            rendered
        };
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Ok(());
        };
        let writer = client.writer.clone();
        let has_graphics = !graphics.assets.is_empty() || !graphics.placements.is_empty();
        let mut next_shell_graphics_delivery = Some(delivery);
        let prepared = client
            .render_state
            .prepare_pane_surface(protocol::PaneSurfaceFrame {
                boot_id: self.client_shell_boot_id.clone(),
                projection_revision: client.shell_projection_revision,
                surface_revision: 0,
                frame,
                panes,
                splits,
                graphics,
            });
        let Some(mut prepared) = prepared else {
            client.clear_deferred_render();
            crate::utils::render::prof::event("full_render.skip_identical");
            return Ok(());
        };
        let mut shell_assets_deferred = false;
        let Some(serialized) = Self::serialize_full_pane_surface(
            client_id,
            &mut prepared,
            has_graphics,
            &mut next_shell_graphics_delivery,
            &mut shell_assets_deferred,
        )?
        else {
            return Ok(());
        };
        let shell_graphics_pending = next_shell_graphics_delivery
            .as_ref()
            .is_some_and(crate::server::rendering::images::DeliveryCache::has_pending);
        match writer.render.try_send(serialized) {
            Ok(()) => {
                if let Some(delivery) = next_shell_graphics_delivery {
                    client.shell_graphics_delivery = delivery;
                }
                client.render_state.commit_sent_frame(prepared);
                if shell_graphics_pending || shell_assets_deferred {
                    client.defer_full_render();
                } else {
                    client.clear_deferred_render();
                }
                crate::utils::render::prof::event("full_render.sent");
            }
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                client.defer_full_render();
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                return Err(());
            }
        }
        Ok(())
    }

    fn serialize_full_pane_surface(
        client_id: u64,
        prepared: &mut crate::server::rendering::stream::PreparedRender,
        has_graphics: bool,
        next_shell_graphics_delivery: &mut Option<crate::server::rendering::images::DeliveryCache>,
        shell_assets_deferred: &mut bool,
    ) -> Result<Option<Vec<u8>>, ()> {
        let max = if has_graphics {
            MAX_GRAPHICS_FRAME_SIZE
        } else {
            crate::protocol::wire::MAX_FRAME_SIZE
        };
        let serialized = match Self::frame_server_message_with_max(prepared.message(), max) {
            Ok(frame) => frame,
            Err(protocol::FramingError::Oversized { claimed, max }) if has_graphics => {
                warn!(
                    client_id,
                    claimed, max, "dropping graphics assets from oversized pane surface"
                );
                if !prepared.strip_pane_surface_assets() {
                    crate::utils::render::prof::event("full_render.serialize_oversized");
                    return Ok(None);
                }
                *next_shell_graphics_delivery = None;
                *shell_assets_deferred = true;
                match Self::frame_server_message(prepared.message()) {
                    Ok(framed) => framed,
                    Err(err) => {
                        warn!(client_id, err = %err, "failed to serialize pane surface without assets");
                        crate::utils::render::prof::event("full_render.serialize_error");
                        return Err(());
                    }
                }
            }
            Err(protocol::FramingError::Oversized { claimed, max }) => {
                warn!(
                    client_id,
                    claimed, max, "skipping oversized frame for client"
                );
                crate::utils::render::prof::event("full_render.serialize_oversized");
                return Ok(None);
            }
            Err(err) => {
                warn!(client_id, err = %err, "failed to serialize frame");
                crate::utils::render::prof::event("full_render.serialize_error");
                return Err(());
            }
        };

        Ok(Some(serialized))
    }
}
