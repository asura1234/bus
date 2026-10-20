//! Coordinator poll ownership.
use super::dialogs;
use super::{
    io, launch, resume, schema, AgentId, BTreeMap, BusState, Duration, Method, Path, PathBuf,
    ResponseResult, RoomAgent, RuntimeStatus, Worker,
};

impl Worker {
    pub(super) fn poll(&mut self) -> Result<Option<Vec<schema::AgentInfo>>, String> {
        let result = self
            .transport
            .request(Method::AgentList(schema::EmptyParams {}));
        let infos = match result {
            Ok(ResponseResult::AgentList { agents }) => agents,
            other => {
                self.poll_failures = self.poll_failures.saturating_add(1);
                tracing::warn!(event = "bus.coordinator.poll_retry",
                    failures = self.poll_failures,
                    retry_ms = self.poll_interval().as_millis() as u64,
                    error = ?other, "Native poll failed; delivery paused, queues and active requests retained");
                // A blocking receive timeout is WouldBlock on macOS. Retry
                // this read-only observation on later ticks, without feeding
                // a transport diagnostic into the composer's sticky error.
                if self.poll_failures >= 3 {
                    self.observe_native_unavailable()?;
                }
                return Ok(None);
            }
        };
        if self.poll_failures > 0 {
            tracing::info!(
                event = "bus.coordinator.poll_recovered",
                failures = self.poll_failures,
                "Native poll recovered"
            );
            self.poll_failures = 0;
        }
        let mut state = self.state.clone();
        let mut rebound = Vec::new();
        let mut waits = Vec::new();
        let dir = &self.data_dir;
        for agent in self.state.agents() {
            let info = infos
                .iter()
                .find(|i| {
                    Some(i.terminal_id.as_str()) == agent.runtime_identity.terminal_id.as_deref()
                        && Some(i.pane_id.as_str()) == agent.runtime_identity.pane_id.as_deref()
                        && i.agent.as_deref() == Some(launch::provider_kind(agent.provider))
                        && agent
                            .runtime_identity
                            .session_id
                            .as_ref()
                            .is_none_or(|expected| {
                                i.agent_session
                                    .as_ref()
                                    .is_some_and(|session| &session.value == expected)
                            })
                })
                .or_else(|| resume::restored_agent(&self.state, agent, &infos));
            if let Some(info) = info {
                if agent.runtime_identity.terminal_id.as_deref() != Some(info.terminal_id.as_str())
                    || agent.runtime_identity.pane_id.as_deref() != Some(info.pane_id.as_str())
                {
                    let mut identity = agent.runtime_identity.clone();
                    identity.terminal_id = Some(info.terminal_id.clone());
                    identity.pane_id = Some(info.pane_id.clone());
                    state
                        .set_agent_runtime_identity(agent.id, identity)
                        .map_err(|e| e.to_string())?;
                    rebound.push((
                        agent.id,
                        agent.runtime_identity.terminal_id.clone(),
                        info.terminal_id.clone(),
                    ));
                }
            }
            let status = polled_status(agent, info);
            observe_exit(self.transport.as_mut(), dir, &mut state, agent, info)?;
            confirm_interactive_agent(&mut state, agent, info)?;
            state
                .observe_status(agent.id, status, io::now_ms())
                .map_err(|e| e.to_string())?;
            let dialog_id = info.and_then(|info| info.dialog_id.clone());
            state
                .observe_dialog(agent.id, dialog_id.is_some())
                .map_err(|e| e.to_string())?;
            if info.is_some() {
                waits.push((
                    agent.id,
                    dialog_id.or_else(|| {
                        (status == RuntimeStatus::Blocked).then(|| dialogs::BLOCKED.to_owned())
                    }),
                ));
            }
            if let Some(info) = info {
                update_polled_metadata(&mut state, agent, info, &mut self.branch_checks)?;
                // The terminal could not install the agent's own update and
                // left its chooser for a person; show why until it is over.
                state
                    .observe_update_error(agent.id, info.update_error.clone())
                    .map_err(|e| e.to_string())?;
            }
        }
        self.apply_poll(state)?;
        for (agent, previous, current) in rebound {
            tracing::info!(event = "bus.resume.rebound", agent_id = agent.0,
                previous_terminal_id = ?previous, terminal_id = current,
                "Reconnected the saved provider conversation; request ownership preserved");
        }
        self.notify_dialogs(waits)?;
        Ok(Some(infos))
    }

    pub(super) fn poll_interval(&self) -> Duration {
        // Do not sleep inside the transport or replay any mutating request.
        // The coordinator can still accept commands between failed polls.
        Duration::from_millis(500 << self.poll_failures.saturating_sub(1).min(2))
    }

    fn observe_native_unavailable(&mut self) -> Result<(), String> {
        let mut state = self.state.clone();
        for id in self.state.agents().map(|a| a.id) {
            state
                .observe_status(id, RuntimeStatus::Unavailable, io::now_ms())
                .map_err(|e| e.to_string())?;
        }
        self.apply_poll(state)
    }
}

fn branch_for(cwd: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C"])
        .arg(cwd)
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!branch.is_empty()).then_some(branch)
}

fn observe_exit(
    transport: &mut dyn super::Transport,
    data_dir: &Path,
    state: &mut BusState,
    agent: &RoomAgent,
    info: Option<&schema::AgentInfo>,
) -> Result<(), String> {
    if agent.deletion_pending {
        return Ok(());
    }
    let evidence = match (
        info,
        agent.runtime_identity.pane_id.as_ref(),
        agent.runtime_identity.terminal_id.as_ref(),
    ) {
        (Some(_), _, _) => Some(false),
        (None, Some(pane), Some(terminal)) => {
            match transport.request(Method::PaneGet(schema::PaneTarget {
                pane_id: pane.clone(),
            })) {
                Ok(ResponseResult::PaneInfo { pane: evidence })
                    if evidence.pane_id == *pane
                        && evidence.terminal_id == *terminal
                        && evidence.agent_process_exited =>
                {
                    Some(true)
                }
                _ => None,
            }
        }
        _ => None,
    };
    if evidence == Some(true) {
        if let (Some(pane), Some(terminal)) = (
            agent.runtime_identity.pane_id.as_deref(),
            agent.runtime_identity.terminal_id.as_deref(),
        ) {
            super::exit_snapshot::save(transport, data_dir, state, agent, pane, terminal)?;
        }
        state.record_worker_exit(agent.id, io::now_ms());
    }
    let snapshot = state.agent(agent.id).and_then(|a| a.exit_snapshot.as_ref());
    let resume = if snapshot.is_some_and(|s| s.terminal_closed) {
        ""
    } else {
        " Inspect the machine to resume it."
    };
    let snapshot = snapshot.map(|s| format!("\n{}", s.notice()));
    let key = format!(
        "exit:{}:{}",
        agent.id.0,
        agent
            .runtime_identity
            .launch_id
            .as_deref()
            .unwrap_or("unknown")
    );
    state
        .attention_episode(
            key,
            evidence,
            agent.id,
            &format!(
                "Agent process exited: {} (agent {}).{resume}{}",
                agent.name,
                agent.id.0,
                snapshot.unwrap_or_default()
            ),
            io::now_ms(),
        )
        .map_err(|error| error.to_string())
}

fn polled_status(agent: &RoomAgent, info: Option<&schema::AgentInfo>) -> RuntimeStatus {
    info.map_or(RuntimeStatus::Unavailable, |info| {
        if agent.session_binding_invalidated || agent.deletion_pending {
            return RuntimeStatus::Unavailable;
        }
        if info.launch_pending {
            return RuntimeStatus::Launching;
        }
        if !info.interactive_ready {
            return RuntimeStatus::Unavailable;
        }
        match info.agent_status {
            schema::AgentStatus::Idle | schema::AgentStatus::Done => RuntimeStatus::Idle,
            schema::AgentStatus::Working => RuntimeStatus::Working,
            schema::AgentStatus::Blocked => RuntimeStatus::Blocked,
            schema::AgentStatus::Unknown => RuntimeStatus::Unavailable,
        }
    })
}

fn confirm_interactive_agent(
    state: &mut BusState,
    agent: &RoomAgent,
    info: Option<&schema::AgentInfo>,
) -> Result<(), String> {
    // Hook installation is explicitly consented before the agent is
    // created. Once the native terminal reports that exact owned
    // agent as interactive, room delivery is ready too; requiring a
    // second Bus-only confirmation creates an Idle-but-undeliverable
    // deadlock (especially for Codex, whose SessionStart is deferred
    // until its first prompt).
    if info.is_some_and(|info| info.interactive_ready)
        && !agent.hook_setup_confirmed
        && !agent.session_binding_invalidated
        && !agent.deletion_pending
    {
        state
            .confirm_hook_setup(agent.id)
            .map_err(|e| e.to_string())?;
        state
            .set_agent_error(agent.id, None)
            .map_err(|e| e.to_string())?;
        tracing::info!(
            event = "bus.agent.ready",
            agent_id = agent.id.0,
            provider = ?agent.provider,
            "Owned provider terminal is interactive; room delivery enabled"
        );
    }
    Ok(())
}

fn update_polled_metadata(
    state: &mut BusState,
    agent: &RoomAgent,
    info: &schema::AgentInfo,
    branch_checks: &mut BTreeMap<AgentId, std::time::Instant>,
) -> Result<(), String> {
    let cwd = info
        .foreground_cwd
        .as_ref()
        .or(info.cwd.as_ref())
        .map(PathBuf::from)
        .unwrap_or_else(|| agent.cwd.clone());
    // Branch lookup only when visible metadata changes; bounded one process per agent per poll is avoided.
    let branch = if cwd != agent.cwd
        || branch_checks
            .get(&agent.id)
            .is_none_or(|time| time.elapsed() >= Duration::from_secs(10))
    {
        branch_checks.insert(agent.id, std::time::Instant::now());
        branch_for(&cwd)
    } else {
        agent.branch.clone()
    };
    state
        .update_agent_runtime_metadata(agent.id, cwd, branch)
        .map_err(|e| e.to_string())?;
    Ok(())
}
