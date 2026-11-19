//! Provider turn facts read beyond the hook's own verdict: background shells
//! for wake ownership, and Codex usage from its rollout.
use super::{
    callbacks, AgentId, BTreeMap, CallbackEventKind, Parsed, Path, PathBuf, ProviderCallback,
    Worker,
};

impl Worker {
    pub(super) fn mark_scheduled_turn(
        state: &mut super::BusState,
        callback: &ProviderCallback,
        record: &callbacks::Record,
    ) -> bool {
        if !record.manifest.provider.uses_claude_harness()
            || !matches!(callback.kind, super::CallbackEventKind::PromptStarted)
        {
            return true;
        }
        use crate::agents::providers::claude_code::hooks::{prompt_origin, PromptOrigin};
        match prompt_origin(&record.value) {
            Some(PromptOrigin::Scheduled | PromptOrigin::Human) => {
                // Explicit input Bus submitted still wins inside this model guard.
                state.mark_unowned_background_turn(callback);
            }
            None if record
                .value
                .get("transcript_path")
                .and_then(serde_json::Value::as_str)
                .is_some()
                && state
                    .agent(callback.agent_id)
                    .and_then(|agent| agent.current_request)
                    .and_then(|id| state.request(id))
                    .is_some_and(|request| request.awaiting_background) =>
            {
                // Keep the hook spooled: an unpublished row is not evidence of
                // an owned continuation without a captured answer.
                tracing::debug!(
                    event = "bus.callback.deferred",
                    reason = "claude_prompt_origin_pending",
                    "Awaiting Claude prompt origin before continuing a textless background pause"
                );
                return false;
            }
            _ => {}
        }
        true
    }

    /// Attest only the main-thread submit that actually bound this request.
    pub(super) fn save_claude_retirement_anchor(
        &self,
        state: &super::BusState,
        id: AgentId,
        record: &callbacks::Record,
    ) {
        use crate::agents::providers::claude_code::retirement::TranscriptAnchor;

        if !record.manifest.provider.uses_claude_harness()
            || record.nested_provider
            || record
                .value
                .get("hook_event_name")
                .and_then(serde_json::Value::as_str)
                != Some("UserPromptSubmit")
        {
            return;
        }
        let Some(agent) = state.agent(id) else { return };
        let Some(request) = agent.current_request.and_then(|id| state.request(id)) else {
            return;
        };
        let runtime = &agent.runtime_identity;
        let hook = |key| record.value.get(key).and_then(serde_json::Value::as_str);
        let (Some(session), Some(prompt), Some(terminal), Some(path)) = (
            runtime.session_id.as_deref(),
            request.provider_prompt_id.as_deref(),
            runtime.terminal_id.as_deref(),
            hook("transcript_path"),
        ) else {
            return;
        };
        if runtime.launch_id.as_deref() != Some(record.manifest.launch_id.as_str())
            || request.expected_launch_id != runtime.launch_id
            || request.provider_session_id.as_deref() != Some(session)
            || hook("session_id") != Some(session)
            || hook("prompt_id") != Some(prompt)
            || !request.trusted_start_bound
        {
            return;
        }
        let Some(anchor) = TranscriptAnchor::from_hook(Path::new(path), session, prompt) else {
            return;
        };
        if anchor
            .save(
                &self.data_dir,
                &record.manifest.launch_id,
                terminal,
                request.id.0,
            )
            .is_err()
        {
            tracing::debug!(event = "bus.callback.retirement_anchor_unavailable");
        }
    }

    /// Observe shell ownership for rejecting inherited wake notifications.
    /// A captured Stop answer is final regardless of these shells.
    pub(super) fn observe_background_shells(
        state: &mut super::BusState,
        id: AgentId,
        record: &callbacks::Record,
        parsed: &mut Parsed,
    ) -> Result<(), String> {
        if !record.manifest.provider.uses_claude_harness() {
            return Ok(());
        }
        let running = matches!(
            parsed,
            Parsed::Final { .. } | Parsed::BackgroundPending { .. }
        )
        .then(|| {
            crate::agents::providers::claude_code::hooks::claude_running_shells(&record.value)
        });
        let owned = state
            .observe_claude_shells(id, running)
            .map_err(|e| e.to_string())?;
        if let Parsed::Final {
            session,
            turn,
            text,
        } = parsed
        {
            if text.trim().is_empty() && owned {
                *parsed = Parsed::BackgroundPending {
                    session: session.clone(),
                    turn: turn.clone(),
                };
            }
        }
        Ok(())
    }

    pub(super) fn is_inherited_shell_wake(
        state: &super::BusState,
        id: AgentId,
        callback: &ProviderCallback,
    ) -> bool {
        let Some(inventory) = state
            .agent(id)
            .and_then(|agent| agent.claude_shells.as_ref())
        else {
            return false;
        };
        let Some(body) = callback
            .prompt_payload
            .as_deref()
            .and_then(crate::agents::providers::claude_code::hooks::task_notification_body)
        else {
            return false;
        };
        let task = body
            .trim()
            .strip_prefix("<task-id>")
            .and_then(|tagged| tagged.split_once("</task-id>").map(|(task, _)| task.trim()));
        task.is_some_and(|task| inventory.inherited.contains(task))
    }

    /// Codex writes rate limits into its rollout after each turn. Usage is
    /// advisory: a failed read keeps the previous snapshot and never errors.
    pub(in crate::messaging::coordinator) fn refresh_codex_usage(
        &mut self,
        id: AgentId,
        path: &Path,
    ) {
        match crate::agents::providers::codex::usage::read_codex_observation(path) {
            Ok(Some(observation)) => {
                if observation.observed_at_ms > crate::utils::time::now_ms()
                    || self
                        .usage
                        .codex
                        .as_ref()
                        .is_some_and(|old| old.read_at_ms >= observation.observed_at_ms)
                {
                    return;
                }
                self.usage.codex = Some(crate::messaging::coordinator::usage::UsageSnapshot {
                    windows: observation.windows,
                    read_at_ms: observation.observed_at_ms,
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

pub(in crate::messaging::coordinator) fn claude_retirement_turn(
    state: &super::BusState,
    agent: AgentId,
    request: crate::messaging::model::RequestId,
) -> Option<crate::protocol::api::schema::ClaudeRetirementTurn> {
    let runtime = &state.agent(agent)?.runtime_identity;
    if !state.group_members(request).is_empty() {
        return None;
    }
    let request = state.request(request)?;
    let launch = runtime.launch_id.as_ref()?;
    let session = runtime.session_id.as_ref()?;
    let prompt = request.provider_prompt_id.as_ref()?;
    (request.trusted_start_bound
        && !prompt.is_empty()
        && request.native_delivery_identity.as_ref() == Some(runtime)
        && request.expected_launch_id.as_ref() == Some(launch)
        && request.provider_session_id.as_ref() == Some(session)
        && request.provider_turn_id.as_ref() == Some(prompt))
    .then(|| crate::protocol::api::schema::ClaudeRetirementTurn {
        launch_id: launch.clone(),
        prompt_id: prompt.clone(),
    })
}

/// Map an observation only after the coordinator has validated its routing facts.
pub(super) fn callback(
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
        provider_prompt_id: record
            .manifest
            .provider
            .uses_claude_harness()
            .then_some(turn),
        prompt_payload: prompt,
        kind,
    }
}

/// Applies turns in the order they began, so a later turn (a task notification,
/// or the user typing) can never overtake the Stop of the turn Bus submitted.
/// Session starts go first, and within a turn the submit hook goes first:
/// companion hooks can reach the spool in either order, including after reconnect.
pub(super) fn order_records(records: &mut Vec<(PathBuf, callbacks::Record)>) {
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
                    | Parsed::CompletionCandidate { session, turn }
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

/// Keep the native enrichment that the ordinary callback tick performs first.
pub(super) fn requires_fresh_poll(record: &callbacks::Record) -> bool {
    match record.manifest.provider {
        super::Provider::Grok | super::Provider::Kimi | super::Provider::Agy => matches!(
            callbacks::parse(record.manifest.provider, &record.value),
            Ok(Parsed::CompletionCandidate { .. } | Parsed::ReconcileSession { .. })
        ),
        super::Provider::Cursor => matches!(
            callbacks::parse(record.manifest.provider, &record.value),
            Ok(Parsed::Failure { error: None, .. })
        ),
        _ => false,
    }
}

/// The hook's process chain, so the server can refuse a report from an agent
/// process it saw exit.
pub(super) fn reporting_processes(
    record: &callbacks::Record,
) -> Vec<super::schema::ReportingProcess> {
    record
        .reporter
        .iter()
        .map(|process| super::schema::ReportingProcess {
            pid: process.pid,
            birth: process.birth,
        })
        .collect()
}
