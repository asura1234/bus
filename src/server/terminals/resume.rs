use std::time::Instant;

use bytes::Bytes;
use ratatui::layout::Rect;

use crate::messaging::coordinator::resume::{LaunchExtras, NativeResumeContext, NativeResumeFacts};
use crate::server::app::App;

fn terminal_resume_facts(terminal: &crate::terminal::TerminalState) -> NativeResumeFacts {
    NativeResumeFacts {
        agent_name: terminal.agent_name.clone(),
        managed_agent: terminal
            .managed_agent_kind()
            .map(crate::agents::agent_label)
            .map(str::to_owned),
        session: terminal.persisted_agent_session.clone(),
    }
}

pub(crate) fn unstarted_restore_plan(
    agent_name: Option<&str>,
    managed_agent: Option<crate::agents::AgentKind>,
    argv: Option<&[String]>,
) -> Option<crate::agents::resume::catalog::AgentResumePlan> {
    let facts = NativeResumeFacts {
        agent_name: agent_name.map(str::to_owned),
        managed_agent: managed_agent
            .map(crate::agents::agent_label)
            .map(str::to_owned),
        session: None,
    };
    crate::messaging::coordinator::resume::unstarted_codex_plan(&facts, argv)
}

fn for_native_resume(
    terminal: &crate::terminal::TerminalState,
    plan: &crate::agents::resume::catalog::AgentResumePlan,
    cwd: &std::path::Path,
) -> Result<LaunchExtras, String> {
    let facts = terminal_resume_facts(terminal);
    if !facts.is_bus_owned() {
        return Ok(LaunchExtras::default());
    }
    // Capture the same env bytes locally, without importing the CLI edge.
    let root = std::env::var_os("BUS_DATA_DIR").map(std::path::PathBuf::from);
    let session_name = crate::utils::paths::active_name();
    let config_root = crate::utils::config::config_dir();
    if !root
        .as_ref()
        .is_some_and(|root| root.is_absolute() && config_root == root.join("herdr-config"))
        || session_name.as_deref() != Some(crate::messaging::coordinator::DEFAULT_SESSION)
    {
        return Ok(LaunchExtras::default());
    }
    let binary = std::env::current_exe().map_err(|_| "Bus resume executable unavailable")?;
    // Project lookup is a native-resume operation, never a rendering read.
    let project = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        .unwrap_or_else(|| cwd.to_path_buf());
    let context = NativeResumeContext {
        root,
        session_name,
        config_root,
        project,
        binary,
    };
    crate::messaging::coordinator::resume::for_native_resume(&context, &facts, plan)
}

/// Map the agents-domain kind into the API's neutral serialized DTO.
pub(crate) fn api_session_kind(
    kind: crate::agents::resume::catalog::AgentSessionRefKind,
) -> crate::protocol::api::schema::AgentSessionRefKind {
    match kind {
        crate::agents::resume::catalog::AgentSessionRefKind::Id => {
            crate::protocol::api::schema::AgentSessionRefKind::Id
        }
        crate::agents::resume::catalog::AgentSessionRefKind::Path => {
            crate::protocol::api::schema::AgentSessionRefKind::Path
        }
    }
}

struct PendingAgentResumeCandidate {
    pane_id: crate::utils::ids::PaneId,
    terminal_id: crate::utils::ids::TerminalId,
    cwd: std::path::PathBuf,
    plan: crate::agents::resume::catalog::AgentResumePlan,
    rows: u16,
    cols: u16,
}

impl App {
    pub(crate) fn has_pending_agent_resumes(&self) -> bool {
        self.state
            .terminals
            .values()
            .any(|terminal| terminal.pending_agent_resume_plan.is_some())
    }

    pub(crate) fn sync_pending_agent_resume_deadline(&mut self, now: Instant) {
        if !self.has_pending_agent_resumes() {
            self.pending_agent_resume_deadline = None;
            return;
        }
        if self.pending_agent_resume_candidates().is_empty() {
            self.pending_agent_resume_deadline = None;
            return;
        }
        self.pending_agent_resume_deadline
            .get_or_insert(now + crate::server::app::PENDING_AGENT_RESUME_THEME_WAIT);
    }

    pub(crate) fn pending_agent_resume_due(&self, now: Instant) -> bool {
        self.pending_agent_resume_deadline
            .is_some_and(|deadline| now >= deadline)
    }

    pub(crate) fn start_pending_agent_resumes(&mut self, allow_empty_theme: bool) -> bool {
        let pending = self.pending_agent_resume_candidates();
        let mut changed = false;
        for PendingAgentResumeCandidate {
            pane_id,
            terminal_id,
            cwd,
            plan,
            rows,
            cols,
        } in pending
        {
            if self.terminal_runtimes.get(&terminal_id).is_some() {
                continue;
            }
            changed |= self.start_pending_agent_resume(
                pane_id,
                terminal_id,
                cwd,
                plan,
                rows,
                cols,
                allow_empty_theme,
            );
        }

        if changed {
            self.schedule_session_save();
        }
        if !self.has_pending_agent_resumes() || self.pending_agent_resume_candidates().is_empty() {
            self.pending_agent_resume_deadline = None;
        }
        changed
    }

    fn pending_agent_resume_candidates(&self) -> Vec<PendingAgentResumeCandidate> {
        let terminal_area = self.state.view.terminal_area;
        if terminal_area.width == 0 || terminal_area.height == 0 {
            return Vec::new();
        };

        let mut pending = Vec::new();
        for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
            for (tab_idx, tab) in ws.tabs.iter().enumerate() {
                for info in
                    self.pending_agent_resume_pane_infos(ws_idx, tab_idx, tab, terminal_area)
                {
                    let Some(pane) = tab.panes.get(&info.id) else {
                        continue;
                    };
                    if self
                        .terminal_runtimes
                        .get(&pane.attached_terminal_id)
                        .is_some()
                    {
                        continue;
                    }
                    let Some(terminal) = self.state.terminals.get(&pane.attached_terminal_id)
                    else {
                        continue;
                    };
                    let Some(plan) = terminal.pending_agent_resume_plan.clone() else {
                        continue;
                    };
                    pending.push(PendingAgentResumeCandidate {
                        pane_id: info.id,
                        terminal_id: pane.attached_terminal_id.clone(),
                        cwd: terminal.cwd.clone(),
                        plan,
                        rows: info.inner_rect.height,
                        cols: info.inner_rect.width,
                    });
                }
            }
        }
        pending
    }

    fn pending_agent_resume_pane_infos(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        tab: &crate::server::workspaces::Tab,
        terminal_area: Rect,
    ) -> Vec<crate::server::workspaces::layout::PaneInfo> {
        let mut pane_infos = derived_pending_agent_resume_pane_infos(
            tab,
            terminal_area,
            self.state.pane_borders,
            self.state.pane_gaps,
            self.state.pane_outer_borders,
        );

        if self.state.active == Some(ws_idx)
            && self
                .state
                .workspaces
                .get(ws_idx)
                .is_some_and(|ws| tab_idx == ws.active_tab_index())
        {
            for visible_info in &self.state.view.pane_infos {
                if let Some(info) = pane_infos
                    .iter_mut()
                    .find(|info| info.id == visible_info.id)
                {
                    *info = visible_info.clone();
                } else {
                    pane_infos.push(visible_info.clone());
                }
            }
        }

        pane_infos
    }

    fn start_pending_agent_resume(
        &mut self,
        pane_id: crate::utils::ids::PaneId,
        terminal_id: crate::utils::ids::TerminalId,
        cwd: std::path::PathBuf,
        plan: crate::agents::resume::catalog::AgentResumePlan,
        rows: u16,
        cols: u16,
        allow_empty_theme: bool,
    ) -> bool {
        let host_terminal_theme = self.state.host_terminal_theme;
        if host_terminal_theme.is_empty() && !allow_empty_theme {
            return false;
        }

        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return false;
        };
        let extras = match for_native_resume(terminal, &plan, &cwd) {
            Ok(extras) => extras,
            Err(reason) => {
                return self.suspend_pending_agent_resume(pane_id, &terminal_id, &reason);
            }
        };
        let plan = match &extras.session {
            Some(session) => {
                let Some(plan) = crate::agents::resume::catalog::plan(
                    &session.source,
                    &session.agent,
                    &session.session_ref,
                ) else {
                    return false;
                };
                tracing::info!(event = "bus.resume.session_corrected", pane = pane_id.raw(),
                    terminal = %terminal_id, session = %session.session_ref.value,
                    "Resuming the conversation Bus bound instead of the terminal's stale one");
                if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
                    terminal.set_persisted_agent_session(session.clone());
                }
                plan
            }
            None => plan,
        };
        let mut argv = plan.argv;
        argv.extend(extras.args);
        let resume_command = shell_command_from_argv(&argv);
        let Some(launch_env) = self
            .find_pane(pane_id)
            .and_then(|(ws_idx, _)| self.pane_launch_env(ws_idx, pane_id, extras.env))
        else {
            return false;
        };

        let runtime = match crate::terminal::TerminalRuntime::spawn(
            pane_id,
            rows,
            cols,
            cwd,
            self.state.pane_scrollback_limit_bytes,
            host_terminal_theme,
            self.state.host_terminal_appearance,
            crate::terminal::runtime::PaneShellConfig::new(
                &self.state.default_shell,
                self.state.shell_mode,
            ),
            &launch_env,
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!(
                    pane = pane_id.raw(),
                    terminal = %terminal_id,
                    agent = %plan.agent,
                    err = %err,
                    "failed to start shell for deferred agent resume"
                );
                // No process ran, so nothing replaced the saved conversation:
                // keep it for a later restart once the shell is fixed.
                self.suspend_pending_agent_resume(pane_id, &terminal_id, "shell failed to start");
                return false;
            }
        };

        let mut input = resume_command;
        input.push('\r');
        if let Err(err) = runtime.try_send_bytes(Bytes::from(input)) {
            tracing::warn!(
                pane = pane_id.raw(),
                terminal = %terminal_id,
                agent = %plan.agent,
                err = %err,
                "failed to send deferred agent resume command to shell"
            );
            runtime.shutdown();
            return false;
        }

        self.terminal_runtimes.insert(terminal_id.clone(), runtime);
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            terminal.pending_agent_resume_plan = None;
            terminal.respawn_shell_on_exit = false;
        }
        true
    }

    fn suspend_pending_agent_resume(
        &mut self,
        pane_id: crate::utils::ids::PaneId,
        terminal_id: &crate::utils::ids::TerminalId,
        reason: &str,
    ) -> bool {
        tracing::warn!(event = "bus.resume.suspended", pane = pane_id.raw(), terminal = %terminal_id, %reason,
            "Bus conversation was kept; resume requires inspection before restarting");
        if let Some(terminal) = self.state.terminals.get_mut(terminal_id) {
            // Keep durable name/session metadata for recovery on a later
            // restart, but do not retry disk reads on every layout tick.
            terminal.pending_agent_resume_plan = None;
            let kind = terminal.managed_agent_kind();
            terminal.set_detected_state_with_screen_signals_at(
                kind,
                crate::agents::AgentState::Unknown,
                false,
                false,
                None,
                Instant::now(),
            );
        }
        true
    }
}

fn derived_pending_agent_resume_pane_infos(
    tab: &crate::server::workspaces::Tab,
    terminal_area: Rect,
    pane_borders: crate::utils::config::PaneBordersConfig,
    pane_gaps: bool,
    pane_outer_borders: bool,
) -> Vec<crate::server::workspaces::layout::PaneInfo> {
    crate::server::rendering::surface::apply_pane_chrome(
        tab.layout.panes(terminal_area),
        pane_borders,
        pane_gaps,
        pane_outer_borders,
    )
    .into_iter()
    .map(|mut info| {
        let pane_inner =
            crate::server::rendering::surface::pane_inner_rect(info.rect, info.borders);
        info.inner_rect = stable_terminal_inner_rect(pane_inner);
        info
    })
    .collect()
}

fn stable_terminal_inner_rect(pane_inner: Rect) -> Rect {
    if pane_inner.width <= 4 {
        return pane_inner;
    }

    Rect::new(
        pane_inner.x,
        pane_inner.y,
        pane_inner.width.saturating_sub(1),
        pane_inner.height,
    )
}

fn shell_command_from_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|part| shell_quote(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b'%' | b'+' | b'='
            )
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
fn terminal_resume_identity_for_test(
    terminal: &crate::terminal::TerminalState,
) -> (
    Option<String>,
    Option<String>,
    Option<crate::agents::resume::catalog::PersistedAgentSession>,
) {
    let facts = terminal_resume_facts(terminal);
    (facts.agent_name, facts.managed_agent, facts.session)
}

#[cfg(test)]
fn native_resume_is_empty_for_test(
    terminal: &crate::terminal::TerminalState,
    plan: &crate::agents::resume::catalog::AgentResumePlan,
    cwd: &std::path::Path,
) -> Result<bool, String> {
    for_native_resume(terminal, plan, cwd).map(|extras| extras == LaunchExtras::default())
}

#[cfg(test)]
#[path = "tests/resume_test.rs"]
mod tests;

#[cfg(test)]
#[path = "resume/test_support_test.rs"]
mod test_support;
