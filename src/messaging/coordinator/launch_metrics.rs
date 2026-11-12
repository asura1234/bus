//! Observe launch outcomes without changing native launch or delivery behavior.
use super::{io, resume, schema, AgentId, BusState, Provider, RoomAgent, RoomId, Worker};
use crate::messaging::model::{AgentRuntimeIdentity, LaunchAttempt, LaunchKind, LaunchOutcome};

impl Worker {
    pub(super) fn begin_tracked_launch(
        &mut self,
        name: &str,
        provider: Provider,
        room: RoomId,
        kind: LaunchKind,
    ) -> Result<u64, String> {
        let mut state = self.state.clone();
        let id = state.begin_launch_attempt(name.trim(), provider, room, kind, io::now_ms());
        self.save(state)?;
        Ok(id)
    }

    pub(super) fn begin_agent_launch(
        &mut self,
        agent: &RoomAgent,
        kind: LaunchKind,
    ) -> Result<u64, String> {
        let mut state = self.state.clone();
        let now = io::now_ms();
        state.cancel_pending_launch(agent.id, "resume_requested", now);
        let id = state.begin_launch_attempt(&agent.name, agent.provider, agent.room_id, kind, now);
        state
            .attach_launch_attempt(id, agent.id)
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(id)
    }

    pub(super) fn track_launch_result<T>(
        &mut self,
        id: u64,
        cause: &str,
        result: Result<T, String>,
    ) -> Result<T, String> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let mut state = self.state.clone();
                let record = state.launch_attempt(id).ok_or("Unknown launch attempt")?;
                let identity = AgentRuntimeIdentity {
                    launch_id: record.launch_id.clone(),
                    terminal_id: record.terminal_id.clone(),
                    ..Default::default()
                };
                state.settle_launch_attempt(
                    id,
                    &identity,
                    LaunchOutcome::Failed,
                    Some(cause),
                    io::now_ms(),
                );
                self.save(state)?;
                Err(error)
            }
        }
    }
}

pub(super) fn bind_current(state: &mut BusState, agent: AgentId) {
    let Some(a) = state.agent(agent) else {
        return;
    };
    let Some(id) = a.launch_attempt_id else {
        return;
    };
    let identity = a.runtime_identity.clone();
    state.bind_launch_identity(id, &identity);
}

pub(super) fn settle_current(
    state: &mut BusState,
    agent: AgentId,
    outcome: LaunchOutcome,
    cause: Option<&str>,
    at_ms: u64,
) {
    let Some(record) = state.current_launch_attempt(agent) else {
        return;
    };
    let id = record.id;
    let identity = AgentRuntimeIdentity {
        launch_id: record.launch_id.clone(),
        terminal_id: record.terminal_id.clone(),
        ..Default::default()
    };
    state.settle_launch_attempt(id, &identity, outcome, cause, at_ms);
}

fn observe_generation(
    state: &mut BusState,
    previous: &RoomAgent,
    agent: &RoomAgent,
    info: Option<&schema::AgentInfo>,
    now: u64,
) -> Result<(), String> {
    let Some(info) = info else {
        return Ok(());
    };
    let current = state.current_launch_attempt(agent.id);
    let new_terminal = previous
        .runtime_identity
        .terminal_id
        .as_ref()
        .is_some_and(|old| old != &info.terminal_id);
    let native_attempt = resume::native_attempt(previous, info).is_some();
    let owned = agent.runtime_identity.terminal_id.as_deref() == Some(info.terminal_id.as_str())
        && agent.runtime_identity.pane_id.as_deref() == Some(info.pane_id.as_str());
    let already_current =
        current.is_some_and(|r| r.terminal_id.as_deref() == Some(info.terminal_id.as_str()));
    let unbound_pending =
        current.is_some_and(|r| r.outcome == LaunchOutcome::Pending && r.terminal_id.is_none());
    if owned
        && !agent.session_binding_invalidated
        && agent.runtime_identity.launch_id.is_some()
        && !already_current
        && !unbound_pending
        && (new_terminal || native_attempt)
    {
        settle_current(
            state,
            agent.id,
            LaunchOutcome::Failed,
            Some("launch_replaced_before_ready"),
            now,
        );
        let id = state.begin_launch_attempt(
            &agent.name,
            agent.provider,
            agent.room_id,
            LaunchKind::NativeResume,
            now,
        );
        state
            .attach_launch_attempt(id, agent.id)
            .map_err(|e| e.to_string())?;
    }
    // An identity already on the agent before this attempt must not fill an
    // unbound record. A still-listed previous terminal would otherwise settle
    // an unsent resume as usable. Absence, or that same listing past the
    // deadline, still times the unbound record out.
    let identity_predates_attempt = unbound_pending
        && previous.runtime_identity.launch_id == agent.runtime_identity.launch_id
        && previous.runtime_identity.terminal_id == agent.runtime_identity.terminal_id;
    if owned && !identity_predates_attempt {
        bind_current(state, agent.id);
    }
    Ok(())
}
fn failure_cause(
    previous: &RoomAgent,
    agent: &RoomAgent,
    info: Option<&schema::AgentInfo>,
    record: &LaunchAttempt,
    now: u64,
) -> Option<&'static str> {
    let fresh_exit = agent.exit_snapshot.as_ref().is_some_and(|s| {
        s.at_ms >= record.started_at_ms
            && s.launch_id == record.launch_id
            && Some(s.terminal_id.as_str()) == record.terminal_id.as_deref()
    });
    if previous.missing_binary.is_none() && agent.missing_binary.is_some() {
        return Some("missing_executable");
    }
    if info.is_some_and(|i| i.launch_input_error.is_some()) {
        return Some("launch_input_failed");
    }
    if info
        .and_then(|i| resume::native_attempt(agent, i))
        .is_some_and(|r| r.error.is_some())
    {
        return Some("native_resume_failed");
    }
    if fresh_exit {
        return Some("unavailable_at_launch");
    }
    if now < record.deadline_at_ms || record.dialog_wait_started_at_ms.is_some() {
        return None;
    }
    Some(if agent.session_binding_invalidated {
        "session_hook_missing"
    } else if info.is_some_and(|i| i.agent_status == schema::AgentStatus::Blocked) {
        "blocked_timeout"
    } else {
        "startup_timeout"
    })
}

/// Called only after a successful native listing. Transport failures projected
/// as Unavailable elsewhere never establish a launch failure here.
pub(super) fn observe(
    state: &mut BusState,
    previous: &RoomAgent,
    info: Option<&schema::AgentInfo>,
    now: u64,
) -> Result<(), String> {
    let agent = state
        .agent(previous.id)
        .ok_or("Unknown launch agent")?
        .clone();
    if agent.deletion_pending || agent.interrupt_pending {
        state.cancel_pending_launch(
            agent.id,
            if agent.deletion_pending {
                "agent_deleted"
            } else {
                "interrupted"
            },
            now,
        );
        return Ok(());
    }
    observe_generation(state, previous, &agent, info, now)?;
    let Some(record) = state.current_launch_attempt(agent.id) else {
        return Ok(());
    };
    if record.outcome != LaunchOutcome::Pending {
        return Ok(());
    }
    let matching = record.launch_id == agent.runtime_identity.launch_id
        && record.terminal_id == agent.runtime_identity.terminal_id;
    if !matching {
        // A lost terminal-create response can leave the saved OLD identity on
        // the agent. Fresh absence, or that same identity still listed, bounds
        // the new unbound attempt. Do not bind it: that would record the
        // unsent resume as usable.
        let previous_identity_still_listed = info.is_some_and(|listed| {
            previous.runtime_identity.launch_id == agent.runtime_identity.launch_id
                && previous.runtime_identity.terminal_id == agent.runtime_identity.terminal_id
                && Some(listed.terminal_id.as_str())
                    == agent.runtime_identity.terminal_id.as_deref()
                && Some(listed.pane_id.as_str()) == agent.runtime_identity.pane_id.as_deref()
        });
        if record.terminal_id.is_none()
            && now >= record.deadline_at_ms
            && (info.is_none() || previous_identity_still_listed)
        {
            settle_current(
                state,
                agent.id,
                LaunchOutcome::Failed,
                Some("startup_timeout"),
                now,
            );
        }
        return Ok(());
    }
    let waiting_on_dialog = info.is_some_and(|i| {
        (i.dialog_id.is_some()
            || i.launch_pending && i.agent_status == schema::AgentStatus::Blocked)
            && Some(i.terminal_id.as_str()) == agent.runtime_identity.terminal_id.as_deref()
            && Some(i.pane_id.as_str()) == agent.runtime_identity.pane_id.as_deref()
    });
    state.observe_launch_dialog(agent.id, waiting_on_dialog, now);
    let Some(record) = state.current_launch_attempt(agent.id) else {
        return Ok(());
    };
    let ready = info.is_some_and(|i| {
        i.agent
            .as_deref()
            .is_some_and(|label| agent.provider.observes_label(label))
            && i.native_ready
            && i.interactive_ready
            && !i.launch_pending
            && i.dialog_id.is_none()
            && matches!(
                i.agent_status,
                schema::AgentStatus::Idle | schema::AgentStatus::Done
            )
    });
    let started = state.launch_message_started(
        agent.id,
        record.launch_id.as_deref(),
        record.submission_revision_floor,
    );
    if started || !agent.session_binding_invalidated && ready {
        // Positive evidence after an observation gap wins over elapsed wall time.
        settle_current(state, agent.id, LaunchOutcome::Usable, None, now);
        return Ok(());
    }
    let cause = failure_cause(previous, &agent, info, record, now);
    if let Some(cause) = cause {
        settle_current(state, agent.id, LaunchOutcome::Failed, Some(cause), now);
    }
    Ok(())
}

/// Called after JsonStore save, including callback batches; log pointers never
/// advertise a record whose write failed. No command or provider text is logged.
pub(super) fn log_outcomes(before: &BusState, after: &BusState) {
    if !after.launch_records_changed_from(before) {
        return;
    }
    for attempt in after.launch_attempts().filter(|a| {
        before
            .launch_attempt(a.id)
            .is_none_or(|old| old.outcome != a.outcome)
    }) {
        tracing::info!(event="bus.agent.launch_outcome",attempt_id=attempt.id,
            agent_id=attempt.agent_id.map(|id|id.0),provider=attempt.provider.label(),
            kind=?attempt.kind,outcome=?attempt.outcome,cause=attempt.cause.as_deref(),
            started_at_ms=attempt.started_at_ms,settled_at_ms=attempt.settled_at_ms,
            dialog_wait_ms=attempt.dialog_wait_ms,
            "Agent launch outcome persisted");
    }
}
