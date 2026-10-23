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
        let observed_agents: Vec<_> = self.state.agents().cloned().collect();
        for agent in &observed_agents {
            let info = polled_agent(&self.state, agent, &infos);
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
            let (missing, exit_evidence) =
                self.observe_missing_binary(&mut state, agent, status, info)?;
            if !missing {
                observe_exit(
                    self.transport.as_mut(),
                    &self.data_dir,
                    &mut state,
                    agent,
                    info,
                    exit_evidence,
                )?;
            }
            confirm_interactive_agent(&mut state, agent, info)?;
            let observed_at_ms = io::now_ms();
            state
                .observe_status(agent.id, status, observed_at_ms)
                .map_err(|e| e.to_string())?;
            let dialog_id = info.and_then(|info| info.dialog_id.clone());
            state
                .observe_dialog(agent.id, dialog_id.is_some(), observed_at_ms)
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
        self.close_exited_terminals()?;
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

/// A pending owned launch has no detected provider yet. It still carries the
/// saved session and managed name; track its terminal before failure clears them.
fn polled_agent<'a>(
    state: &BusState,
    agent: &RoomAgent,
    infos: &'a [schema::AgentInfo],
) -> Option<&'a schema::AgentInfo> {
    infos
        .iter()
        .find(|i| {
            Some(i.terminal_id.as_str()) == agent.runtime_identity.terminal_id.as_deref()
                && Some(i.pane_id.as_str()) == agent.runtime_identity.pane_id.as_deref()
                && (i.agent.as_deref() == Some(launch::provider_kind(agent.provider))
                    || i.agent.is_none()
                        && i.launch_pending
                        && i.name.as_deref()
                            == Some(
                                crate::messaging::identity::managed_name(agent.room_id, agent.id)
                                    .as_str(),
                            ))
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
        .or_else(|| resume::restored_agent(state, agent, infos))
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
    exit_evidence: Option<bool>,
) -> Result<(), String> {
    if agent.deletion_pending {
        return Ok(());
    }
    // The saved exit of this launch stands once Bus closed the pane that
    // showed it, so later requests still learn the worker exited.
    let saved = info.is_none()
        && agent
            .exit_snapshot
            .as_ref()
            .is_some_and(|s| s.launch_id == agent.runtime_identity.launch_id);
    let evidence = match (
        info,
        agent.runtime_identity.pane_id.as_ref(),
        agent.runtime_identity.terminal_id.as_ref(),
    ) {
        _ if saved => Some(true),
        (Some(_), _, _) => Some(false),
        (None, Some(_), Some(_)) => exit_evidence,
        _ => None,
    };
    if evidence == Some(true) {
        if let (Some(pane), Some(terminal)) = (
            agent.runtime_identity.pane_id.as_deref(),
            agent.runtime_identity.terminal_id.as_deref(),
        ) {
            super::exit_snapshot::save(transport, data_dir, state, agent, pane, terminal)?;
        }
        let snapshot = state.agent(agent.id).and_then(|a| a.exit_snapshot.as_ref());
        if snapshot.is_some_and(|s| s.close_pending()) {
            // Reported once the saved state is durable and the close is done.
            return Ok(());
        }
    }
    report_exit(state, agent.id, evidence)
}

/// Records the exit for open requests and diagnostic logs; the saved snapshot
/// remains available through lifecycle state.
pub(super) fn report_exit(
    state: &mut BusState,
    id: AgentId,
    evidence: Option<bool>,
) -> Result<(), String> {
    if evidence == Some(true) {
        state.record_worker_exit(id, io::now_ms());
    }
    let agent = state.agent(id).ok_or("Unknown agent")?;
    if evidence == Some(true) {
        let name = agent.name.clone();
        let launch = agent
            .runtime_identity
            .launch_id
            .clone()
            .unwrap_or_else(|| "unknown".into());
        let snapshot = agent.exit_snapshot.clone();
        if state.record_error_episode(format!("exit:{}:{launch}", id.0)) {
            tracing::warn!(event = "bus.agent.process_exited", agent_id = id.0, agent_name = name, launch_id = launch, exit_snapshot_path = ?snapshot.as_ref().and_then(|s| s.path.as_deref()), snapshot_error = ?snapshot.as_ref().and_then(|s| s.error.as_deref()), terminal_closed = ?snapshot.as_ref().and_then(|s| s.terminal_closed), "Agent process exited; inspect lifecycle state");
        }
    }
    Ok(())
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
