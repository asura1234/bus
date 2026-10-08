//! Shell respawn policy after an agent or launch command exits.
use crate::server::app::App;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
const WINDOWS_POWERSHELL_AGENT_EXIT_RESPAWN_GRACE: Duration = Duration::from_secs(2);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::server) enum RuntimeExitAction {
    RespawnShell,
    ClosePane,
}

impl App {
    pub(in crate::server) fn runtime_exit_action(
        &self,
        pane_id: crate::server::workspaces::layout::PaneId,
    ) -> RuntimeExitAction {
        let Some((_, pane_state)) = self.find_pane(pane_id) else {
            return RuntimeExitAction::ClosePane;
        };
        let Some(terminal) = self.state.terminals.get(&pane_state.attached_terminal_id) else {
            return RuntimeExitAction::ClosePane;
        };

        if terminal.respawn_shell_on_exit || self.should_respawn_shell_after_agent_exit(terminal) {
            RuntimeExitAction::RespawnShell
        } else {
            RuntimeExitAction::ClosePane
        }
    }

    fn should_respawn_shell_after_agent_exit(
        &self,
        terminal: &crate::terminal::TerminalState,
    ) -> bool {
        #[cfg(not(windows))]
        {
            let _ = terminal;
            false
        }

        #[cfg(windows)]
        {
            if !terminal.agent_process_exited_within(
                Instant::now(),
                WINDOWS_POWERSHELL_AGENT_EXIT_RESPAWN_GRACE,
            ) {
                return false;
            }

            crate::pane::uses_windows_powershell_pane_shell(crate::pane::PaneShellConfig::new(
                &self.state.default_shell,
                self.state.shell_mode,
            ))
        }
    }

    pub(in crate::server) fn respawn_shell_for_launch_pane(
        &mut self,
        pane_id: crate::server::workspaces::layout::PaneId,
        focus_pane: bool,
    ) -> bool {
        let Some((ws_idx, pane_state)) = self.find_pane(pane_id) else {
            return false;
        };
        let terminal_id = pane_state.attached_terminal_id.clone();
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return false;
        };

        let cwd = terminal.cwd.clone();
        let (rows, cols) = self
            .terminal_runtimes
            .get(&terminal_id)
            .map(|runtime| runtime.current_size())
            .unwrap_or_else(|| self.state.estimate_pane_size());
        let Some(launch_env) = self.pane_launch_env(ws_idx, pane_id, Vec::new()) else {
            return false;
        };
        let runtime = match crate::terminal::TerminalRuntime::spawn(
            pane_id,
            rows,
            cols,
            cwd,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            self.state.host_terminal_appearance,
            crate::pane::PaneShellConfig::new(&self.state.default_shell, self.state.shell_mode),
            &launch_env,
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!(target: "bus::server::api",
                    pane = pane_id.raw(),
                    terminal = %terminal_id,
                    err = %err,
                    "failed to respawn shell after launch command exited"
                );
                return false;
            }
        };

        self.terminal_runtimes.insert(terminal_id.clone(), runtime);
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            terminal.clear_agent_runtime_identity_after_respawn();
        }
        if focus_pane {
            self.state.focus_pane_in_workspace(ws_idx, pane_id);
        }
        self.schedule_session_save();
        true
    }
}
