use super::{runtime_hosts_agent, App};

impl App {
    pub(in crate::server) fn agent_info(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
    ) -> Option<crate::protocol::api::schema::AgentInfo> {
        self.agent_info_with_detection(ws_idx, pane_id, false)
    }

    pub(super) fn agent_info_with_detection(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
        cached: bool,
    ) -> Option<crate::protocol::api::schema::AgentInfo> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let pane_state = ws.pane_state(pane_id)?;
        let terminal = self.state.terminals.get(&pane_state.attached_terminal_id)?;
        if !terminal.is_agent_terminal() && terminal.native_resume.is_none() {
            return None;
        }
        // AgentInfo does not expose scroll metrics; querying them would wait on VT.
        let pane = self.pane_info_without_scroll(ws_idx, pane_id)?;
        let screen = screen_metadata(
            self.lookup_runtime_sender(ws_idx, pane_id),
            pane.agent.as_deref(),
            pane.agent_status == crate::protocol::api::schema::AgentStatus::Working,
            cached,
        );
        let quota = screen.as_ref().is_some_and(|s| s.provider_quota);
        Some(crate::protocol::api::schema::AgentInfo {
            generation: self.state.generation,
            launch_input_error: self
                .lookup_runtime_sender(ws_idx, pane_id)
                .and_then(|runtime| runtime.definite_launch_input_error()),
            native_resume: terminal.native_resume.as_ref().map(|resume| {
                crate::protocol::api::schema::NativeResumeInfo {
                    name: resume.name.clone(),
                    agent: crate::agents::agent_label(resume.kind).into(),
                    session_id: resume
                        .session
                        .as_ref()
                        .map(|session| session.session_ref.value.clone()),
                    command: resume.command.clone(),
                    error: resume.error.clone(),
                }
            }),
            terminal_id: pane.terminal_id,
            name: terminal.agent_name.clone(),
            agent: pane.agent,
            title: pane.title,
            terminal_title: pane.terminal_title,
            terminal_title_stripped: pane.terminal_title_stripped,
            display_agent: pane.display_agent,
            agent_status: if cached
                && screen.is_none()
                && matches!(
                    pane.agent_status,
                    crate::protocol::api::schema::AgentStatus::Idle
                        | crate::protocol::api::schema::AgentStatus::Done
                ) {
                crate::protocol::api::schema::AgentStatus::Unknown
            } else {
                pane.agent_status
            },
            screen_detection_skipped: screen.is_none(),
            compaction_banner: screen.as_ref().is_some_and(|s| s.compaction_banner),
            dialog_id: screen.and_then(|s| s.dialog),
            update_error: None,
            provider_error: quota.then(|| {
                crate::agents::providers::cursor::usage::MONTHLY_USAGE_LIMIT_ERROR.to_owned()
            }),
            provider_quota_reset_at: quota.then(|| {
                crate::agents::providers::cursor::usage::monthly_reset_at(
                    crate::utils::time::now_ms(),
                )
            }),
            state_labels: pane.state_labels,
            tokens: pane.tokens,
            agent_session: pane.agent_session,
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            pane_id: pane.pane_id,
            focused: pane.focused,
            launch_pending: terminal.pending_agent_resume_plan.is_some()
                || terminal.managed_agent_launch_pending(),
            interactive_ready: terminal.pending_agent_resume_plan.is_none()
                && terminal.managed_agent_interactive_ready(),
            native_ready: terminal.pending_agent_resume_plan.is_none()
                && !terminal.managed_agent_launch_pending()
                && terminal.effective_known_agent().is_some_and(|expected| {
                    self.lookup_runtime_sender(ws_idx, pane_id)
                        .is_some_and(|runtime| {
                            let hosts = runtime_hosts_agent(runtime, expected);
                            if hosts {
                                runtime.reconcile_launch_attempt(true);
                            }
                            hosts
                        })
                }),
            state_change_seq: terminal.last_agent_state_change_seq.unwrap_or(0),
            cwd: pane.cwd,
            foreground_cwd: pane.foreground_cwd,
            revision: pane.revision,
        })
    }
}

struct ScreenMetadata {
    compaction_banner: bool,
    provider_quota: bool,
    dialog: Option<String>,
}

fn screen_metadata(
    runtime: Option<&crate::terminal::TerminalRuntime>,
    agent: Option<&str>,
    working: bool,
    cached: bool,
) -> Option<ScreenMetadata> {
    let runtime = runtime?;
    if cached {
        let snapshot =
            runtime.cached_screen_detection(agent.and_then(crate::agents::parse_agent_label))?;
        return Some(ScreenMetadata {
            compaction_banner: snapshot.compaction_banner,
            provider_quota: snapshot.provider_quota,
            dialog: snapshot.dialog(working),
        });
    }
    // Explicit single-agent reads and input guards retain fresh observation.
    let (grid, _) = crate::terminal::driver::observe(runtime).ok()?;
    let text = grid.text();
    Some(ScreenMetadata {
        compaction_banner: crate::agents::compaction_banner::visible(agent, &text),
        provider_quota: agent == Some("cursor")
            && crate::agents::providers::cursor::usage::is_restored_monthly_usage_limit(&text),
        dialog: crate::terminal::driver::blocker(&grid, working),
    })
}
