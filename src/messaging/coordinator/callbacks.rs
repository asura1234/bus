//! Durable provider callback consumption and request correlation.
use super::{
    launch, schema, AgentId, Author, BTreeMap, BTreeSet, BusState, CallbackDisposition,
    CallbackEventKind, CallbackRejection, Method, Path, PathBuf, Provider, ProviderCallback,
    ResponseResult, RoomId, RuntimeStatus, Worker,
};

#[cfg(test)]
use super::{AgentRuntimeIdentity, Draft, Transport};
use crate::agents::providers::{spool as callbacks, spool::Parsed, ProviderKind};
use std::ops::ControlFlow;

type CallbackRecordResult = ControlFlow<(), (BusState, Option<ProviderCallback>, Vec<PathBuf>)>;
type CursorCallbackResult = ControlFlow<(), (BusState, ProviderCallback, PathBuf)>;

enum CallbackFlow {
    Next,
    Stop,
}

#[path = "reports.rs"]
mod reports;

impl Worker {
    /// A native hook can bind its session before the Bus spool is consumed.
    /// During deletion, reconcile only that first launch-attested identity;
    /// never publish native metadata, process replies, or clear suspension.
    pub(super) fn reconcile_deleting_initial_session(
        &mut self,
        id: AgentId,
    ) -> Result<bool, String> {
        let agent = self.state.agent(id).ok_or("Unknown callback agent")?;
        if !agent.deletion_pending
            || agent.session_binding_invalidated
            || agent.runtime_identity.session_id.is_some()
        {
            return Ok(false);
        }
        let Some(launch) = &agent.runtime_identity.launch_id else {
            return Ok(false);
        };
        let records = callbacks::records(&self.data_dir.join("callbacks").join(launch))
            .map_err(|e| e.to_string())?;
        let mut sessions = BTreeSet::new();
        for (_, record) in records {
            let Ok(Parsed::Session { session, .. }) =
                callbacks::parse(agent.provider, &record.value)
            else {
                continue;
            };
            if record.manifest.routing_key != callbacks::RoutingKey(id.0)
                || record.manifest.provider != agent.provider
                || &record.manifest.launch_id != launch
            {
                return self.agent_error(id, "Session-start callback launch identity mismatch; deletion remains suspended.".into()).map(|_| false);
            }
            sessions.insert(session);
        }
        if sessions.len() > 1 {
            return self
                .agent_error(
                    id,
                    "Conflicting initial session-start callbacks; deletion remains suspended."
                        .into(),
                )
                .map(|_| false);
        }
        let Some(session) = sessions.into_iter().next() else {
            return Ok(false);
        };
        let mut identity = agent.runtime_identity.clone();
        identity.session_id = Some(session);
        let mut state = self.state.clone();
        state
            .set_agent_runtime_identity(id, identity)
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(true)
    }

    /// The provider session the native server attributes to `pane`.
    pub(super) fn native_session(&mut self, pane: &str) -> Result<Option<String>, String> {
        match self
            .transport
            .request(Method::AgentGet(schema::AgentTarget {
                target: pane.to_owned(),
            }))
            .map_err(|e| e.message)?
        {
            ResponseResult::AgentInfo { agent } => Ok(agent.agent_session.map(|s| s.value)),
            _ => Err("Unexpected native agent response".into()),
        }
    }

    pub(super) fn consume_callbacks(&mut self, id: AgentId, dir: &Path) -> Result<(), String> {
        let mut rollout = None;
        let mut records = callbacks::records(dir).map_err(|e| e.to_string())?;
        order_records(&mut records);
        for (path, record) in &records {
            if matches!(
                self.consume_callback_record(id, dir, path, record, &records, &mut rollout)?,
                CallbackFlow::Stop
            ) {
                return Ok(());
            }
        }
        if let Some(path) = rollout {
            self.refresh_codex_usage(id, &path);
        }
        if let Some(agent) = self.state.agent(id) {
            self.usage.refresh_claude(agent, dir);
        }
        Ok(())
    }

    fn apply_request_callback(
        &mut self,
        id: AgentId,
        state: &mut BusState,
        callback: ProviderCallback,
    ) -> Result<bool, String> {
        if matches!(
            callback.kind,
            CallbackEventKind::Final { .. } | CallbackEventKind::BackgroundPending
        ) && !state.is_unrelated_turn(&callback)
            && state
                .agent(id)
                .and_then(|a| a.current_request)
                .and_then(|r| state.request(r))
                .is_some_and(|r| {
                    !r.trusted_start_bound
                        && callback.sequence > r.submission_boundary.unwrap_or(u64::MAX)
                })
        {
            // It may also end a turn the agent started on its own before
            // the submit hook ran, so it is not an agent error.
            tracing::debug!(
                event = "bus.callback.deferred",
                reason = "trusted_start_missing",
                "Stop retained until matching submit hook arrives"
            );
            return Ok(false);
        }
        let started = matches!(callback.kind, CallbackEventKind::PromptStarted);
        let report = reports::pending_report(state, &callback);
        let disposition = state.accept_callback(callback);
        if let Some(report) = report {
            reports::post_report(
                state,
                report,
                &disposition,
                crate::messaging::storage::io::now_ms(),
            );
        }
        // A turn the agent began on its own keeps it busy even while the
        // native status still reads idle; settling it clears that.
        match (&disposition, started) {
            (
                CallbackDisposition::Rejected(
                    CallbackRejection::UnrelatedTurn | CallbackRejection::NoActiveRequest,
                ),
                true,
            ) => {
                self.own_turns.insert(id, std::time::Instant::now());
            }
            (_, false) => {
                self.own_turns.remove(&id);
            }
            _ => {}
        }
        tracing::info!(event = "bus.callback.correlated", disposition = ?disposition,
            "Provider callback correlation result");
        if disposition == CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn) {
            tracing::info!(
                event = "bus.callback.unrelated_turn",
                "Agent activity outside the Bus request"
            );
        }
        if matches!(
            disposition,
            CallbackDisposition::Rejected(
                CallbackRejection::UnboundFinal | CallbackRejection::WrongPrompt
            )
        ) {
            state.set_agent_error(id,Some("Unbound provider callback: verify trusted submit/stop hooks. The active request will not be retried.".into())).map_err(|e|e.to_string())?;
        }
        Ok(true)
    }

    fn cursor_final_callback(
        &mut self,
        id: AgentId,
        mut state: BusState,
        record: &callbacks::Record,
        records: &[(PathBuf, callbacks::Record)],
        session: String,
        turn: String,
    ) -> Result<CursorCallbackResult, String> {
        let agent = self.state.agent(id).ok_or("Unknown callback agent")?;
        let found = records.iter().find_map(|(other_path, other)| {
            match callbacks::parse(agent.provider, &other.value) {
                Ok(Parsed::Response {
                    session: s,
                    turn: t,
                    ..
                }) if s == session
                    && t == turn
                    && other.manifest.launch_id == record.manifest.launch_id =>
                {
                    Some((other_path, other))
                }
                _ => None,
            }
        });
        let Some((other_path, response)) = found else {
            tracing::debug!(
                event = "bus.callback.deferred",
                reason = "cursor_response_missing",
                "Awaiting response companion"
            );
            return Ok(ControlFlow::Break(()));
        };
        // 转录对不上时，停止钩子已完成且代理空闲，就用钩子正文结算。
        let idle = self
            .state
            .agent(id)
            .is_some_and(|agent| agent.status == RuntimeStatus::Idle);
        let text =
            match crate::agents::providers::cursor::final_reply::settle_text(&response.value, idle)
            {
                Ok(text) => text,
                Err(message) => {
                    tracing::debug!(
                        event = "bus.callback.deferred",
                        reason = "cursor_transcript_pending",
                        "Awaiting completed Cursor transcript"
                    );
                    if agent.actionable_error.as_deref() != Some(message.as_str()) {
                        state
                            .set_agent_error(id, Some(message))
                            .map_err(|e| e.to_string())?;
                        self.save(state)?;
                    }
                    return Ok(ControlFlow::Break(()));
                }
            };
        Ok(ControlFlow::Continue((
            state,
            callback(
                record,
                id,
                session,
                turn,
                None,
                CallbackEventKind::Final { text },
            ),
            other_path.clone(),
        )))
    }

    fn bind_session_callback(
        &mut self,
        id: AgentId,
        state: &mut BusState,
        record: &callbacks::Record,
        session: String,
        source: Option<String>,
    ) -> Result<(), String> {
        let agent = self.state.agent(id).ok_or("Unknown callback agent")?;
        let mut identity = agent.runtime_identity.clone();
        let pane = identity
            .pane_id
            .clone()
            .ok_or("Callback arrived before pane creation was recorded")?;
        self.transport
            .request(Method::PaneReportAgentSession(
                schema::PaneReportAgentSessionParams {
                    pane_id: pane,
                    // Use the existing provider adapter namespace:
                    // a generic source is deliberately rejected by
                    // the native session identity consumer.
                    source: format!("herdr:{}", launch::provider_kind(agent.provider)),
                    agent: launch::provider_kind(agent.provider).into(),
                    seq: Some(record.sequence),
                    agent_session_id: Some(session.clone()),
                    agent_session_path: None,
                    session_start_source: Some("startup".into()),
                },
            ))
            .map_err(|e| e.message)?;
        identity.session_id = Some(session.clone());
        state
            .set_agent_runtime_identity(id, identity)
            .map_err(|e| e.to_string())?;
        if agent.session_reset_pending {
            // A reset agent that had no session yet binds its first one here.
            state
                .rebind_reset_session(id, session)
                .map_err(|e| e.to_string())?;
        }
        // Identical hooks share one spool file, so a hook retry counts once.
        if source.as_deref() == Some("compact") {
            state
                .record_compaction_callback(
                    id,
                    &record.manifest.launch_id,
                    record.sequence,
                    record.at_ms,
                )
                .map_err(|e| e.to_string())?;
        }
        if agent.current_request.is_none() {
            state
                .set_agent_error(
                    id,
                    if agent.hook_setup_confirmed {
                        None
                    } else {
                        Some("Not ready; waiting for the provider terminal.".into())
                    },
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn callback_from_record(
        &mut self,
        id: AgentId,
        mut state: BusState,
        path: &Path,
        record: &callbacks::Record,
        records: &[(PathBuf, callbacks::Record)],
        parsed: Parsed,
    ) -> Result<CallbackRecordResult, String> {
        let mut remove = vec![path.to_path_buf()];
        let callback = match parsed {
            Parsed::Session { session, source } => {
                self.bind_session_callback(id, &mut state, record, session, source)?;
                None
            }
            Parsed::Started {
                session,
                turn,
                prompt,
            } => Some(callback(
                record,
                id,
                session,
                turn,
                Some(prompt),
                CallbackEventKind::PromptStarted,
            )),
            Parsed::Final {
                session,
                turn,
                text,
            } => Some(callback(
                record,
                id,
                session,
                turn,
                None,
                CallbackEventKind::Final { text },
            )),
            Parsed::BackgroundPending { session, turn } => Some(callback(
                record,
                id,
                session,
                turn,
                None,
                CallbackEventKind::BackgroundPending,
            )),
            Parsed::Failure {
                session,
                turn,
                message,
            } => Some(callback(
                record,
                id,
                session,
                turn,
                None,
                CallbackEventKind::Error { message },
            )),
            Parsed::Completed { session, turn } => {
                return self
                    .cursor_final_callback(id, state, record, records, session, turn)
                    .map(|flow| {
                        flow.map_continue(|(state, callback, companion)| {
                            remove.push(companion);
                            (state, Some(callback), remove)
                        })
                    });
            }
            Parsed::Response { .. } => return Ok(ControlFlow::Break(())),
            Parsed::Ignore => None,
        };
        Ok(ControlFlow::Continue((state, callback, remove)))
    }

    fn rebind_callback_session(
        &mut self,
        id: AgentId,
        session: &str,
        parsed: &Parsed,
        record: &callbacks::Record,
        pane: String,
        path: &Path,
        dir: &Path,
        mut state: BusState,
    ) -> Result<CallbackFlow, String> {
        // `agent clear` started a fresh provider context in this
        // terminal. Claude and Codex announce it with SessionStart;
        // Cursor's first new turn names its new conversation.
        let source = match parsed {
            Parsed::Session { source, .. } => source.clone(),
            _ => None,
        };
        self.transport
            .request(Method::PaneReportAgentSession(
                schema::PaneReportAgentSessionParams {
                    pane_id: pane.clone(),
                    source: format!("herdr:{}", launch::provider_kind(record.manifest.provider)),
                    agent: launch::provider_kind(record.manifest.provider).into(),
                    seq: Some(record.sequence),
                    agent_session_id: Some(session.to_owned()),
                    agent_session_path: None,
                    session_start_source: Some(source.unwrap_or_else(|| "new".into())),
                },
            ))
            .map_err(|e| e.message)?;
        // The server answers ok even when it keeps the old session,
        // as a server older than this build does for a Cursor new
        // chat. Rebinding then would leave Bus and the terminal
        // disagreeing, so the agent could not be read or deleted.
        if self.native_session(&pane)?.as_deref() != Some(session) {
            tracing::warn!(
                event = "bus.callback.rejected",
                reason = "session_reset_refused",
                "Native server kept the previous provider session"
            );
            state
                .invalidate_agent_session(id)
                .map_err(|e| e.to_string())?;
            state
                .observe_status(
                    id,
                    RuntimeStatus::Unavailable,
                    crate::messaging::storage::io::now_ms(),
                )
                .map_err(|e| e.to_string())?;
            state.set_agent_error(id, Some("The Bus server kept the previous provider session after agent clear, so Bus can no longer follow this agent. Delete it and add a new one; restart Bus first if its server predates this build.".into())).map_err(|e| e.to_string())?;
            self.save(state)?;
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
            crate::platform::sync_parent_directory(dir).map_err(|e| e.to_string())?;
            return Ok(CallbackFlow::Next);
        }
        tracing::info!(
            event = "bus.callback.rebound",
            reason = "session_reset",
            "Agent rebound to its fresh provider session"
        );
        state
            .rebind_reset_session(id, session.to_owned())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        // The callback stays spooled and is consumed on the next pass,
        // now under the rebound session.
        Ok(CallbackFlow::Stop)
    }

    fn check_callback_session(
        &mut self,
        id: AgentId,
        dir: &Path,
        path: &Path,
        record: &callbacks::Record,
        parsed: &Parsed,
        mut state: BusState,
    ) -> Result<ControlFlow<CallbackFlow, BusState>, String> {
        let agent = self.state.agent(id).ok_or("Unknown callback agent")?;
        let callback_session = match parsed {
            Parsed::Session { session, .. }
            | Parsed::Started { session, .. }
            | Parsed::Final { session, .. }
            | Parsed::BackgroundPending { session, .. }
            | Parsed::Failure { session, .. }
            | Parsed::Completed { session, .. }
            | Parsed::Response { session, .. } => Some(session),
            Parsed::Ignore => None,
        };
        if let Some(session) = callback_session {
            if agent
                .runtime_identity
                .session_id
                .as_ref()
                .is_some_and(|known| known != session)
                && agent.session_reset_pending
            {
                let pane = agent
                    .runtime_identity
                    .pane_id
                    .clone()
                    .ok_or("Callback arrived before pane creation was recorded")?;
                return self
                    .rebind_callback_session(id, session, parsed, record, pane, path, dir, state)
                    .map(ControlFlow::Break);
            }
            if agent
                .runtime_identity
                .session_id
                .as_ref()
                .is_some_and(|known| known != session)
            {
                tracing::warn!(
                    event = "bus.callback.rejected",
                    reason = "session_mismatch",
                    "Callback does not own this terminal session"
                );
                // Retain the attested identity even on uncertain sends. Clearing
                // it would let a repeated foreign SessionStart silently rebind.
                if matches!(parsed, Parsed::Session { .. }) {
                    state
                        .invalidate_agent_session(id)
                        .map_err(|e| e.to_string())?;
                }
                state
                    .observe_status(
                        id,
                        RuntimeStatus::Unavailable,
                        crate::messaging::storage::io::now_ms(),
                    )
                    .map_err(|e| e.to_string())?;
                state.set_agent_error(id, Some("Provider callback session differs from this Bus launch; request ownership is preserved. Inspect the terminal and create a new Bus agent for a new session.".into())).map_err(|e| e.to_string())?;
                self.save(state)?;
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
                crate::platform::sync_parent_directory(dir).map_err(|e| e.to_string())?;
                return Ok(ControlFlow::Break(CallbackFlow::Next));
            }
            if agent.runtime_identity.session_id.is_none()
                && !matches!(parsed, Parsed::Session { .. })
            {
                tracing::debug!(
                    event = "bus.callback.deferred",
                    reason = "session_start_missing",
                    "Callback retained"
                );
                // Do not bind a turn before this launch's SessionStart attests
                // which provider session owns the terminal.
                return Ok(ControlFlow::Break(CallbackFlow::Next));
            }
        }
        Ok(ControlFlow::Continue(state))
    }

    fn parse_observed_callback(
        &mut self,
        id: AgentId,
        record: &callbacks::Record,
    ) -> Result<ControlFlow<CallbackFlow, Parsed>, String> {
        let agent = self.state.agent(id).ok_or("Unknown callback agent")?;
        tracing::debug!(
            event = "bus.callback.observed",
            "Reading spooled provider callback"
        );
        if record.manifest.routing_key != callbacks::RoutingKey(id.0)
            || record.manifest.provider != agent.provider
            || Some(record.manifest.launch_id.as_str())
                != agent.runtime_identity.launch_id.as_deref()
        {
            tracing::warn!(
                event = "bus.callback.rejected",
                reason = "launch_identity_mismatch",
                "Callback not accepted"
            );
            return self
                .agent_error(id, "Callback launch identity mismatch".into())
                .map(|()| ControlFlow::Break(CallbackFlow::Stop));
        }
        if agent.session_binding_invalidated {
            tracing::debug!(
                event = "bus.callback.deferred",
                reason = "session_invalidated",
                "Callback retained"
            );
            return Ok(ControlFlow::Break(CallbackFlow::Stop));
        }
        let parsed = match callbacks::parse(agent.provider, &record.value) {
            Ok(parsed) => parsed,
            Err(message) => {
                tracing::warn!(
                    event = "bus.callback.rejected",
                    reason = "invalid_payload",
                    "Callback retained for inspection"
                );
                let mut state = self.state.clone();
                state
                    .set_agent_error(id, Some(message))
                    .map_err(|e| e.to_string())?;
                self.save(state)?;
                return Ok(ControlFlow::Break(CallbackFlow::Next));
            }
        };
        tracing::debug!(
            event = "bus.callback.parsed",
            kind = parsed.kind(),
            "Callback parsed"
        );
        Ok(ControlFlow::Continue(parsed))
    }

    fn consume_callback_record(
        &mut self,
        id: AgentId,
        dir: &Path,
        path: &Path,
        record: &callbacks::Record,
        records: &[(PathBuf, callbacks::Record)],
        rollout: &mut Option<PathBuf>,
    ) -> Result<CallbackFlow, String> {
        let agent = self.state.agent(id).ok_or("Unknown callback agent")?;
        let _span = tracing::info_span!("bus.callback", agent_id = id.0,
            room_id = agent.room_id.0, request_id = ?agent.current_request.map(|r| r.0),
            callback_id = %record.id, sequence = record.sequence,
            launch_id = %record.manifest.launch_id)
        .entered();
        let parsed = match self.parse_observed_callback(id, record)? {
            ControlFlow::Continue(parsed) => parsed,
            ControlFlow::Break(flow) => return Ok(flow),
        };
        let state = self.state.clone();
        let state = match self.check_callback_session(id, dir, path, record, &parsed, state)? {
            ControlFlow::Continue(state) => state,
            ControlFlow::Break(flow) => return Ok(flow),
        };
        if record.manifest.provider == Provider::Codex && matches!(parsed, Parsed::Final { .. }) {
            *rollout = crate::agents::providers::codex::usage::codex_rollout_path(&record.value)
                .map(Path::to_path_buf)
                .or(rollout.take());
        }
        let (mut state, callback, remove) =
            match self.callback_from_record(id, state, path, record, records, parsed)? {
                ControlFlow::Continue(ready) => ready,
                ControlFlow::Break(()) => return Ok(CallbackFlow::Next),
            };
        if let Some(callback) = callback {
            if !self.apply_request_callback(id, &mut state, callback)? {
                return Ok(CallbackFlow::Next);
            }
        }
        self.save(state)?;
        tracing::debug!(
            event = "bus.callback.applied",
            "Callback state persisted before spool acknowledgement"
        );
        for path in remove {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        crate::platform::sync_parent_directory(dir).map_err(|e| e.to_string())?;
        Ok(CallbackFlow::Next)
    }

    /// Codex writes rate limits into its rollout after each turn. Usage is
    /// advisory: a failed read keeps the previous snapshot and never errors.
    fn refresh_codex_usage(&mut self, id: AgentId, path: &Path) {
        match crate::agents::providers::codex::usage::read_codex_rollout(path) {
            Ok(Some(windows)) => {
                self.usage.codex = Some(crate::messaging::coordinator::usage::UsageSnapshot {
                    windows,
                    read_at_ms: crate::messaging::storage::io::now_ms(),
                    observed_by_agent: id,
                });
            }
            Ok(None) => tracing::debug!(
                event = "bus.usage.unavailable",
                agent_id = id.0,
                "Codex rollout has no rate limits yet"
            ),
            Err(error) => tracing::debug!(
                event = "bus.usage.unavailable",
                agent_id = id.0,
                %error,
                "Codex rollout unreadable"
            ),
        }
    }
}

/// Applies turns in the order they began, so a later turn (a task notification,
/// or the user typing) can never overtake the Stop of the turn Bus submitted.
/// Session starts go first, and within a turn the submit hook goes first:
/// companion hooks can reach the spool in either order, including after reconnect.
fn order_records(records: &mut Vec<(PathBuf, callbacks::Record)>) {
    let mut keyed: Vec<_> = std::mem::take(records)
        .into_iter()
        .map(|entry| {
            let parsed = callbacks::parse(entry.1.manifest.provider, &entry.1.value);
            let turn = match &parsed {
                Ok(
                    Parsed::Started { session, turn, .. }
                    | Parsed::Final { session, turn, .. }
                    | Parsed::BackgroundPending { session, turn }
                    | Parsed::Response { session, turn, .. }
                    | Parsed::Completed { session, turn }
                    | Parsed::Failure { session, turn, .. },
                ) => Some((session.clone(), turn.clone())),
                _ => None,
            };
            let rank = match parsed {
                Ok(Parsed::Session { .. }) => 0,
                Ok(Parsed::Started { .. }) => 1,
                _ => 2,
            };
            (rank, turn, entry)
        })
        .collect();
    let mut began = BTreeMap::new();
    for (_, turn, (_, record)) in &keyed {
        if let Some(turn) = turn {
            let first = began.entry(turn.clone()).or_insert(record.sequence);
            *first = (*first).min(record.sequence);
        }
    }
    keyed.sort_by_key(|(rank, turn, (_, record))| {
        let turn_began = turn
            .as_ref()
            .and_then(|turn| began.get(turn).copied())
            .unwrap_or(record.sequence);
        (*rank != 0, turn_began, *rank, record.sequence)
    });
    records.extend(keyed.into_iter().map(|(_, _, entry)| entry));
}

/// Map an observation only after the coordinator has validated its routing facts.
fn callback(
    record: &callbacks::Record,
    id: AgentId,
    session: String,
    turn: String,
    prompt: Option<String>,
    kind: CallbackEventKind,
) -> ProviderCallback {
    ProviderCallback {
        callback_id: record.id.clone(),
        sequence: record.sequence,
        occurred_at_ms: record.at_ms,
        agent_id: id,
        launch_id: record.manifest.launch_id.clone(),
        provider_session_id: Some(session),
        provider_turn_id: Some(turn.clone()),
        provider_prompt_id: (record.manifest.provider == ProviderKind::ClaudeCode).then_some(turn),
        prompt_payload: prompt,
        kind,
    }
}

#[cfg(test)]
#[path = "tests/callbacks_test.rs"]
mod tests;
