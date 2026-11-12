//! Coordinator poll ownership.
use super::dialogs;
use super::{
    io, resume, schema, AgentId, BTreeMap, BusState, Duration, Path, PathBuf, ResponseResult,
    RoomAgent, RuntimeStatus, Worker,
};
use crate::messaging::model::DeliveryEpoch;

impl Worker {
    pub(super) fn poll(&mut self) -> Result<Option<Vec<schema::AgentInfo>>, String> {
        let result = self
            .transport
            .request(super::Method::AgentListWithDetectionContext(
                detection_contexts(&self.state, io::now_ms()),
            ));
        let Some(infos) = self.accept_agent_list(result)? else {
            return Ok(None);
        };
        let mut state = self.state.clone();
        // Retain the previous observation as legacy evidence until this poll's
        // migrated state is durable; a failed save must not erase that boundary.
        migrate_legacy_requests(&mut state, &infos, self.serving_generation)?;
        let mut rebound = Vec::new();
        let mut waits = Vec::new();
        let observed_agents: Vec<_> = self.state.agents().cloned().collect();
        for agent in &observed_agents {
            let previous_agent = agent;
            let info = polled_agent(&self.state, agent, &infos);
            rebound.extend(note_rebind(
                &mut state,
                agent,
                info,
                self.serving_generation,
            )?);
            if let Some(info) = info {
                super::startup::watch_native_resume(&mut state, agent, info)?;
            }
            let current_agent = state.agent(agent.id).ok_or("Unknown polled agent")?.clone();
            let agent = &current_agent;
            let observed_at_ms = self.now_ms();
            self.observe_cursor_info(&mut state, agent, info, observed_at_ms)?;
            let status = if state.awaiting_restart_resume(agent.id)
                && info.is_none_or(|info| !info.native_ready || !info.interactive_ready)
            {
                RuntimeStatus::Unavailable
            } else {
                polled_status(agent, info)
            };
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
            if !info.is_some_and(|info| pending_launch_screen(agent, info)) {
                super::launch_metrics::observe(&mut state, previous_agent, info, observed_at_ms)?;
            }
            // Missing screen facts are not a fresh idle/busy observation.
            if status != agent.status || !info.is_some_and(inconclusive_screen) {
                state
                    .observe_status(agent.id, status, observed_at_ms)
                    .map_err(|e| e.to_string())?;
            }
            Self::observe_grok_banner(&mut state, agent, info, observed_at_ms)?;
            let dialog_id = info.and_then(|info| info.dialog_id.clone());
            if info.is_some_and(|info| !info.screen_detection_skipped) {
                state
                    .observe_dialog(agent.id, dialog_id.is_some(), observed_at_ms)
                    .map_err(|e| e.to_string())?;
                waits.push((
                    agent.id,
                    dialog_id.or_else(|| {
                        (status == RuntimeStatus::Blocked).then(|| dialogs::BLOCKED.to_owned())
                    }),
                ));
            }
            if let Some(info) = info {
                update_polled_metadata(&mut state, agent, info, &mut self.branch_checks)?;
                // Preserve the native compatibility error until an observation
                // clears it, without changing unrelated agent errors.
                state
                    .observe_update_error(agent.id, info.update_error.clone())
                    .map_err(|e| e.to_string())?;
            }
        }
        self.directory_checks.poll(&mut state)?;
        self.apply_poll(state)?;
        note_serving_generation(&mut self.serving_generation, &infos);
        self.close_exited_terminals()?;
        log_rebinds(rebound);
        self.notify_dialogs(waits)?;
        Ok(Some(infos))
    }

    fn observe_grok_banner(
        state: &mut BusState,
        agent: &RoomAgent,
        info: Option<&schema::AgentInfo>,
        observed_at_ms: u64,
    ) -> Result<(), String> {
        // Grok prints a compaction banner instead of Claude's compact hook.
        // A skipped screen read is not "banner gone".
        if agent.provider != super::Provider::Grok {
            return Ok(());
        }
        let Some(info) = info else {
            return Ok(());
        };
        if info.screen_detection_skipped {
            return Ok(());
        }
        state
            .observe_compaction_banner(agent.id, info.compaction_banner, observed_at_ms)
            .map_err(|error| error.to_string())
    }

    fn observe_cursor_info(
        &mut self,
        state: &mut BusState,
        agent: &RoomAgent,
        info: Option<&schema::AgentInfo>,
        observed_at_ms: u64,
    ) -> Result<(), String> {
        if agent.provider == super::Provider::Cursor {
            if let Some(info) = info {
                self.usage.observe_cursor(
                    agent.id,
                    info.provider_error.as_deref(),
                    info.provider_quota_reset_at,
                    observed_at_ms,
                    agent.current_request,
                );
                if let Some(error) = info.provider_error.as_deref() {
                    state
                        .record_bound_provider_error(agent.id, error, observed_at_ms)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }

    fn accept_agent_list(
        &mut self,
        result: Result<ResponseResult, crate::messaging::native::TransportError>,
    ) -> Result<Option<Vec<schema::AgentInfo>>, String> {
        match result {
            Ok(ResponseResult::AgentList { agents }) => {
                if self.poll_failures > 0 {
                    tracing::info!(
                        event = "bus.coordinator.poll_recovered",
                        failures = self.poll_failures,
                        "Native poll recovered"
                    );
                    self.poll_failures = 0;
                }
                Ok(Some(agents))
            }
            other => {
                self.poll_failures = self.poll_failures.saturating_add(1);
                tracing::warn!(event = "bus.coordinator.poll_retry",
                    failures = self.poll_failures,
                    retry_ms = self.poll_interval().as_millis() as u64,
                    error = ?other, "Native poll failed; delivery paused, queues and active requests retained");
                // WouldBlock is the macOS read timeout. Inconclusive polls
                // retain the last status; only connection loss or explicit
                // server unavailability supports the mass transition.
                if self.poll_failures >= 3
                    && matches!(&other, Err(error) if matches!(error.code.as_deref(),
                        Some(crate::messaging::native::NATIVE_SERVER_UNAVAILABLE | "server_unavailable")))
                {
                    self.observe_native_unavailable()?;
                }
                Ok(None)
            }
        }
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

fn note_serving_generation(serving: &mut u64, infos: &[schema::AgentInfo]) {
    if let Some(generation) = infos.iter().map(|info| info.generation).max() {
        *serving = generation;
    }
}

fn log_rebinds(rebound: Vec<(AgentId, Option<String>, String)>) {
    for (agent, previous, current) in rebound {
        tracing::info!(event = "bus.resume.rebound", agent_id = agent.0,
            previous_terminal_id = ?previous, terminal_id = current,
            "Reconnected the saved provider conversation; request ownership preserved");
    }
}

fn migrate_legacy_requests(
    state: &mut BusState,
    infos: &[schema::AgentInfo],
    generation: u64,
) -> Result<(), String> {
    let generation_advanced =
        generation > 0 && infos.iter().any(|info| info.generation > generation);
    let legacy: Vec<_> = state
        .unsettled_requests()
        .filter(|request| {
            request.delivery_epoch.is_none()
                && matches!(
                    request.phase,
                    super::RequestPhase::Submitting | super::RequestPhase::Active
                )
        })
        .map(|request| (request.id, request.agent_id))
        .collect();
    for (id, agent) in legacy {
        let owner = state.agent(agent).ok_or("Unknown legacy agent")?;
        let info = polled_agent(state, owner, infos);
        if info.is_none() && !generation_advanced {
            continue;
        }
        let (Some(terminal), Some(pane)) = (
            &owner.runtime_identity.terminal_id,
            &owner.runtime_identity.pane_id,
        ) else {
            continue;
        };
        let epoch = DeliveryEpoch {
            generation: if generation_advanced {
                generation
            } else if let Some(info) =
                info.filter(|info| terminal == &info.terminal_id && pane == &info.pane_id)
            {
                info.generation
            } else {
                0
            },
            terminal_id: terminal.clone(),
            pane_id: pane.clone(),
        };
        state
            .stamp_delivery_epoch(id, epoch)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn note_rebind(
    state: &mut BusState,
    agent: &RoomAgent,
    info: Option<&schema::AgentInfo>,
    serving_generation: u64,
) -> Result<Vec<(AgentId, Option<String>, String)>, String> {
    let Some(info) = info else {
        return Ok(Vec::new());
    };
    if agent.runtime_identity.terminal_id.as_deref() == Some(info.terminal_id.as_str())
        && agent.runtime_identity.pane_id.as_deref() == Some(info.pane_id.as_str())
    {
        return Ok(Vec::new());
    }
    Ok(vec![rebind_agent(state, agent, info, serving_generation)?])
}

fn rebind_agent(
    state: &mut BusState,
    agent: &RoomAgent,
    info: &schema::AgentInfo,
    serving_generation: u64,
) -> Result<(AgentId, Option<String>, String), String> {
    let previous = agent.runtime_identity.terminal_id.clone();
    let mut identity = agent.runtime_identity.clone();
    identity.terminal_id = Some(info.terminal_id.clone());
    identity.pane_id = Some(info.pane_id.clone());
    state
        .set_agent_runtime_identity(agent.id, identity)
        .map_err(|error| error.to_string())?;
    // A generation advance with a new terminal id is a whole-session restart:
    // the saved pane died with the previous server. The same terminal still
    // has to be close-verified, including on the first poll from generation 0.
    if info.generation > serving_generation {
        if let Some(previous) = previous.filter(|previous| previous != &info.terminal_id) {
            state
                .record_session_restart_closed_terminal(agent.id, previous)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok((
        agent.id,
        agent.runtime_identity.terminal_id.clone(),
        info.terminal_id.clone(),
    ))
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
                && (i
                    .agent
                    .as_deref()
                    .is_some_and(|label| agent.provider.observes_label(label))
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
        && agent.exit_snapshot.as_ref().is_some_and(|s| {
            s.launch_id == agent.runtime_identity.launch_id
                && Some(s.terminal_id.as_str()) == agent.runtime_identity.terminal_id.as_deref()
                && agent
                    .startup
                    .as_ref()
                    .is_none_or(|startup| s.at_ms >= startup.started_at_ms)
        });
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
    if state.agent(id).is_some_and(|a| a.startup.is_some()) {
        return Ok(());
    }
    if evidence == Some(true) {
        state.await_restart_resume(id);
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

// A missing live frame cannot close a dialog wait or prove a launch timeout.
// Explicit input/resume errors and saved exits still reach their existing owner.
fn pending_launch_screen(agent: &RoomAgent, info: &schema::AgentInfo) -> bool {
    info.screen_detection_skipped
        && info.agent_status == schema::AgentStatus::Unknown
        && info.native_ready
        && info.launch_input_error.is_none()
        && info
            .native_resume
            .as_ref()
            .is_none_or(|resume| resume.error.is_none())
        && !agent.exit_snapshot.as_ref().is_some_and(|exit| {
            exit.launch_id == agent.runtime_identity.launch_id
                && Some(exit.terminal_id.as_str()) == agent.runtime_identity.terminal_id.as_deref()
        })
}

fn inconclusive_screen(info: &schema::AgentInfo) -> bool {
    info.screen_detection_skipped
        && info.agent_status == schema::AgentStatus::Unknown
        && info.native_ready
        && info.interactive_ready
        && !info.launch_pending
}

fn polled_status(agent: &RoomAgent, info: Option<&schema::AgentInfo>) -> RuntimeStatus {
    info.map_or(RuntimeStatus::Unavailable, |info| {
        if agent.session_binding_invalidated || agent.deletion_pending {
            return RuntimeStatus::Unavailable;
        }
        if info.agent_status == schema::AgentStatus::Blocked {
            return RuntimeStatus::Blocked;
        }
        if info.launch_pending {
            return RuntimeStatus::Launching;
        }
        if !info.interactive_ready {
            return RuntimeStatus::Unavailable;
        }
        if inconclusive_screen(info) {
            return agent.status;
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

/// Keep request ownership independent of screen-derived state: blocked work
/// remains an obligation, while finished turns and idle drafts do not.
fn detection_contexts(state: &BusState, now: u64) -> schema::AgentDetectionContexts {
    let obligations: std::collections::BTreeSet<_> = state
        .requests()
        .filter(|request| {
            matches!(
                request.phase,
                super::RequestPhase::Queued
                    | super::RequestPhase::Submitting
                    | super::RequestPhase::Active
            )
        })
        .map(|request| request.agent_id)
        .collect();
    let agents = state
        .agents()
        .filter_map(|agent| {
            let identity = &agent.runtime_identity;
            let obligation = obligations.contains(&agent.id);
            let activity_age_ms = agent
                .current_request
                .and_then(|id| state.request(id))
                .filter(|request| {
                    request.trusted_start_bound
                        && request.turn_ended_at_ms.is_none()
                        && request.pending_final.is_none()
                })
                .and_then(|request| request.progress_at_ms)
                .map(|at| now.saturating_sub(at));
            Some(schema::AgentDetectionContext {
                terminal_id: identity.terminal_id.clone()?,
                pane_id: identity.pane_id.clone()?,
                session_id: identity.session_id.clone(),
                obligation,
                activity_age_ms,
            })
        })
        .collect();
    schema::AgentDetectionContexts { agents }
}

#[cfg(test)]
mod detection_context_tests {
    use super::*;
    use crate::messaging::model::{
        AgentRuntimeIdentity, Author, Draft, Provider, SubmissionOutcome,
    };

    #[test]
    fn poll_sends_queued_obligations_through_the_transport_request_contract() {
        use crate::messaging::native::{Transport, TransportError};
        use crate::protocol::api::schema::Method;
        use std::sync::{Arc, Mutex};

        struct Capture(Arc<Mutex<Vec<Method>>>);
        impl Transport for Capture {
            fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
                self.0.lock().unwrap().push(method.clone());
                Ok(match method {
                    Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                        ResponseResult::AgentList { agents: Vec::new() }
                    }
                    _ => ResponseResult::Ok {},
                })
            }
        }
        let root = crate::utils::test_temp::unique_temp_path("bus-poll-context");
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut worker = Worker::open(root.clone(), Box::new(Capture(captured.clone()))).unwrap();
        let mut state = worker.state.clone();
        let room = state.create_room("work").unwrap();
        let agent = state
            .create_agent(room, "worker", Provider::Kimi, root.clone(), None)
            .unwrap();
        state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some("launch".into()),
                    terminal_id: Some("terminal".into()),
                    pane_id: Some("pane".into()),
                    session_id: None,
                },
            )
            .unwrap();
        state
            .submit_message_from(
                room,
                Draft {
                    text: "work".into(),
                    recipient_ids: [agent].into(),
                    files: Vec::new(),
                },
                Author::Developer,
                io::now_ms(),
            )
            .unwrap();
        worker.save(state).unwrap();
        worker.poll().unwrap();
        let methods = captured.lock().unwrap();
        let Some(Method::AgentListWithDetectionContext(context)) = methods.first() else {
            panic!(
                "coordinator discarded detection context at the request boundary: {:?}",
                methods.first()
            );
        };
        assert_eq!(
            context.agents,
            [schema::AgentDetectionContext {
                terminal_id: "terminal".into(),
                pane_id: "pane".into(),
                session_id: None,
                obligation: true,
                activity_age_ms: None,
            }]
        );
        drop(methods);
        drop(worker);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_submitting_and_current_requests_remain_detection_obligations() {
        let mut state = BusState::new();
        let room = state.create_room("work").unwrap();
        let agent = state
            .create_agent(room, "worker", Provider::Kimi, "/tmp".into(), None)
            .unwrap();
        state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some("launch".into()),
                    terminal_id: Some("terminal".into()),
                    pane_id: Some("pane".into()),
                    session_id: None,
                },
            )
            .unwrap();
        assert!(!detection_contexts(&state, 100).agents[0].obligation);
        let request = state
            .submit_message_from(
                room,
                Draft {
                    text: "work".into(),
                    recipient_ids: [agent].into(),
                    files: Vec::new(),
                },
                Author::Developer,
                100,
            )
            .unwrap()[0];
        assert!(detection_contexts(&state, 100).agents[0].obligation);
        state.begin_submission(request, "launch", 1).unwrap();
        assert!(detection_contexts(&state, 100).agents[0].obligation);
        state
            .record_submission(
                request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: None,
                    provider_turn_id: None,
                },
            )
            .unwrap();
        assert!(detection_contexts(&state, 100).agents[0].obligation);
        assert_eq!(
            detection_contexts(&state, 100).agents[0].activity_age_ms,
            None,
            "a submitted request is not proof that its provider turn started"
        );
    }
}

#[cfg(test)]
mod inconclusive_tests {
    use super::*;
    use crate::messaging::model::AgentRuntimeIdentity;
    use crate::messaging::native::{Transport, TransportError};
    use crate::protocol::api::schema::Method;

    struct PendingScreen;
    impl Transport for PendingScreen {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            assert!(matches!(method, Method::AgentListWithDetectionContext(_)));
            let info = serde_json::from_value(serde_json::json!({
                "terminal_id":"terminal", "pane_id":"pane", "agent":"kimi",
                "workspace_id":"w", "tab_id":"t", "focused":false,
                "agent_status":"unknown", "screen_detection_skipped":true,
                "interactive_ready":true, "native_ready":true, "revision":1
            }))
            .unwrap();
            Ok(ResponseResult::AgentList { agents: vec![info] })
        }
    }

    #[test]
    fn inconclusive_publication_preserves_status_clock_and_dialog() {
        let root = crate::utils::test_temp::unique_temp_path("bus-pending-screen");
        let mut worker = Worker::open(root.clone(), Box::new(PendingScreen)).unwrap();
        let mut state = worker.state.clone();
        let room = state.create_room("work").unwrap();
        let agent = state
            .create_agent(
                room,
                "worker",
                super::super::Provider::Kimi,
                root.clone(),
                None,
            )
            .unwrap();
        state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some("launch".into()),
                    terminal_id: Some("terminal".into()),
                    pane_id: Some("pane".into()),
                    session_id: None,
                },
            )
            .unwrap();
        state.confirm_hook_setup(agent).unwrap();
        let launch = state.begin_launch_attempt(
            "worker",
            super::super::Provider::Kimi,
            room,
            crate::messaging::model::LaunchKind::Create,
            1,
        );
        state.attach_launch_attempt(launch, agent).unwrap();
        let identity = state.agent(agent).unwrap().runtime_identity.clone();
        state.bind_launch_identity(launch, &identity);
        state
            .observe_status(agent, RuntimeStatus::Blocked, 1234)
            .unwrap();
        state.observe_dialog(agent, true, 1234).unwrap();
        worker.save(state).unwrap();
        worker.poll().unwrap();
        let current = worker.state.agent(agent).unwrap();
        assert_eq!(
            worker.state.launch_attempt(launch).unwrap().outcome,
            crate::messaging::model::LaunchOutcome::Pending,
            "a live inconclusive frame must not expire the launch"
        );
        assert_eq!(current.status, RuntimeStatus::Blocked);
        assert_eq!(
            current.observed_at_ms, 1234,
            "missing facts must not refresh an idle-inference clock"
        );
        assert!(current.dialog, "missing facts cannot close an open dialog");
        assert!(
            worker.dialog_seen.is_empty(),
            "an inconclusive read starts no wait episode"
        );
        let mut lost: schema::AgentInfo = serde_json::from_value(serde_json::json!({
            "terminal_id":"terminal", "pane_id":"pane", "agent":"kimi",
            "workspace_id":"w", "tab_id":"t", "focused":false,
            "agent_status":"unknown", "screen_detection_skipped":true,
            "interactive_ready":false, "native_ready":false, "revision":1
        }))
        .unwrap();
        assert_eq!(
            polled_status(current, Some(&lost)),
            RuntimeStatus::Unavailable
        );
        lost.interactive_ready = true;
        assert_eq!(
            polled_status(current, Some(&lost)),
            RuntimeStatus::Unavailable,
            "a lost runtime cannot inherit prior availability"
        );
        lost.native_ready = true;
        lost.launch_pending = true;
        assert_eq!(
            polled_status(current, Some(&lost)),
            RuntimeStatus::Launching
        );
        drop(worker);
        std::fs::remove_dir_all(root).unwrap();
    }
}
