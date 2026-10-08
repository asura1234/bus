use super::connection::latest_shell_client;
use crate::config;
use crate::protocol::ServerMessage;
use crate::server::main_loop::HeadlessServer;
use ratatui::layout::Rect;

impl HeadlessServer {
    pub(in crate::server) fn allocate_activity_stamp(&mut self) -> u64 {
        let stamp = self.next_activity_stamp;
        self.next_activity_stamp = self.next_activity_stamp.saturating_add(1);
        stamp
    }

    pub(in crate::server) fn resize_shared_runtime_to_effective_size_before_input(&mut self) {
        self.resize_shared_runtime_to_effective_size_with_pending_agent_resumes(false);
    }

    pub(in crate::server) fn resize_shared_runtime_to_effective_size_with_pending_agent_resumes(
        &mut self,
        start_pending_agent_resumes: bool,
    ) {
        let Some(client_id) = self.foreground_client_id else {
            return;
        };
        self.resize_shell_tab_if_controller(client_id, start_pending_agent_resumes);
    }

    pub(in crate::server) fn sync_runtime_view_geometry(&mut self) {
        crate::ui::compute_view_without_resizing_panes(
            &mut self.app.state,
            &self.app.terminal_runtimes,
            Rect::new(0, 0, self.effective_size.0, self.effective_size.1),
        );
    }

    pub(in crate::server) fn sync_foreground_client_state(&mut self) {
        self.app.pixel_mouse_available = self.foreground_client_id.is_some_and(|id| {
            self.clients
                .get(&id)
                .is_some_and(|client| client.pixel_mouse)
        });
        let Some(client_id) = self.foreground_client_id else {
            self.effective_size = self.headless_size;
            self.app.state.outer_terminal_focus = None;
            self.app.state.host_cell_size = crate::kitty_graphics::HostCellSize::default();
            self.sync_runtime_view_geometry();
            return;
        };
        let Some(client) = self.clients.get(&client_id) else {
            self.foreground_client_id = None;
            self.effective_size = self.headless_size;
            self.app.state.outer_terminal_focus = None;
            self.app.state.host_cell_size = crate::kitty_graphics::HostCellSize::default();
            self.sync_runtime_view_geometry();
            return;
        };

        let terminal_size = client.terminal_size;
        let host_cell_size = if self.app.state.kitty_graphics_enabled && client.cell_size.is_known()
        {
            client.cell_size
        } else {
            crate::kitty_graphics::HostCellSize::default()
        };
        let host_terminal_theme = client.host_terminal_theme;
        let host_terminal_appearance = client.host_terminal_appearance;
        let host_terminal_appearance_explicit = client.host_terminal_appearance_explicit;
        let outer_terminal_focus = client.outer_terminal_focus;

        self.effective_size = terminal_size;
        self.sync_runtime_view_geometry();
        self.app.state.outer_terminal_focus = outer_terminal_focus;
        self.app.state.host_cell_size = host_cell_size;
        if outer_terminal_focus == Some(true) {
            self.app.state.mark_active_tab_seen();
        }
        self.app.set_host_terminal_appearance_state(
            host_terminal_appearance,
            host_terminal_appearance_explicit,
        );
        self.app.set_host_terminal_theme(host_terminal_theme);
    }

    pub(in crate::server) fn reload_server_config(
        &mut self,
        notify_success: bool,
    ) -> crate::config::ConfigReloadReport {
        let report = self.app.apply_config_from_disk(notify_success);
        self.app.take_config_reloaded_from_disk();
        self.headless_size = self.app.state.headless_size;
        self.server_config_diagnostic = config::config_diagnostic_summary(&report.diagnostics);
        self.sync_foreground_client_state();
        report
    }

    pub(in crate::server) fn foreground_client_outer_focus(&self) -> Option<bool> {
        let client_id = self.foreground_client_id?;
        self.clients.get(&client_id)?.outer_terminal_focus
    }

    pub(in crate::server) fn active_tab_suppresses_notifications(
        &self,
        is_active_tab: bool,
    ) -> bool {
        crate::app::actions::active_tab_suppresses_notifications(
            is_active_tab,
            self.foreground_client_outer_focus(),
        )
    }

    pub(in crate::server) fn promote_client_to_foreground(&mut self, client_id: u64) -> bool {
        let stamp = self.allocate_activity_stamp();
        let Some(client) = self.clients.get_mut(&client_id) else {
            return false;
        };
        if !client.shell_surface_active {
            return false;
        }
        client.last_activity = stamp;

        let changed = self.foreground_client_id != Some(client_id);
        self.foreground_client_id = Some(client_id);
        self.sync_foreground_client_state();
        changed
    }

    pub(in crate::server) fn promote_latest_remaining_client(&mut self) -> bool {
        let next_foreground = latest_shell_client(&self.clients);
        let changed = next_foreground != self.foreground_client_id;
        self.foreground_client_id = next_foreground;
        self.sync_foreground_client_state();
        changed
    }

    pub(in crate::server) fn app_client_count(&self) -> usize {
        self.clients
            .values()
            .filter(|client| client.is_active_shell_client())
            .count()
    }

    pub(in crate::server) fn drain_client_config_reload_request(&mut self) {
        if !self.app.state.request_client_config_reload {
            return;
        }
        self.app.state.request_client_config_reload = false;
        self.send_to_all_clients(ServerMessage::ReloadSoundConfig);
    }
}
