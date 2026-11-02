//! Durable launch audit records, including attempts rejected before roster insertion.
use super::{
    AgentId, AgentRuntimeIdentity, BusState, ModelError, Provider, Request, RequestPhase, RoomId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) fn request_pre_start_failure(request: &Request) -> bool {
    matches!(
        request
            .failure_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("missing_reason"),
        "submission_unconfirmed"
            | "prompt_not_shown"
            | "delivery_error"
            | "prior_request_active"
            | "not_submitted"
            | "session_hook_missing"
    )
}

pub(crate) fn request_start_bound(state: &BusState, request: &Request) -> bool {
    request.trusted_start_bound
        || (!request.uncertain_outcome
            && (request.phase == RequestPhase::Active
                || request.phase == RequestPhase::Abandoned && !request.steered)
            && request
                .group
                .and_then(|id| state.request(id))
                .is_some_and(|lead| {
                    lead.agent_id == request.agent_id
                        && !request_pre_start_failure(lead)
                        && (lead.trusted_start_bound
                            || lead.reply().is_some()
                            || lead.phase == RequestPhase::Completed)
                }))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LaunchKind {
    Create,
    Resume,
    Recovery,
    NativeResume,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LaunchOutcome {
    Pending,
    Usable,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LaunchAttempt {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) provider: Provider,
    pub(crate) room_id: RoomId,
    pub(crate) kind: LaunchKind,
    pub(crate) agent_id: Option<AgentId>,
    pub(crate) launch_id: Option<String>,
    pub(crate) terminal_id: Option<String>,
    pub(crate) started_at_ms: u64,
    /// Input guarded by an observation from this attempt has this revision or later.
    #[serde(default)]
    pub(crate) submission_revision_floor: u64,
    pub(crate) deadline_at_ms: u64,
    pub(crate) settled_at_ms: Option<u64>,
    pub(crate) outcome: LaunchOutcome,
    pub(crate) cause: Option<String>,
}

impl BusState {
    /// Audit IDs are independent of roster IDs: rejected preflight must not alias
    /// a subsequently created agent. No record is ever pruned or overwritten.
    pub(crate) fn begin_launch_attempt(
        &mut self,
        name: &str,
        provider: Provider,
        room: RoomId,
        kind: LaunchKind,
        at_ms: u64,
    ) -> u64 {
        let id = self
            .launch_attempts
            .last_key_value()
            .map_or(1, |(last, _)| {
                last.checked_add(1)
                    .expect("Launch audit ID space exhausted")
            });
        self.launch_tracking_started_at_ms.get_or_insert(at_ms);
        Arc::make_mut(&mut self.launch_attempts).insert(
            id,
            LaunchAttempt {
                id,
                name: name.into(),
                provider,
                room_id: room,
                kind,
                agent_id: None,
                launch_id: None,
                terminal_id: None,
                started_at_ms: at_ms,
                submission_revision_floor: self.next_status_revision,
                deadline_at_ms: at_ms
                    .saturating_add(crate::protocol::api::AGENT_START_TIMEOUT.as_millis() as u64),
                settled_at_ms: None,
                outcome: LaunchOutcome::Pending,
                cause: None,
            },
        );
        id
    }

    pub(crate) fn attach_launch_attempt(
        &mut self,
        id: u64,
        agent: AgentId,
    ) -> Result<(), ModelError> {
        let a = self
            .agents
            .get(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?;
        let record = self
            .launch_attempts
            .get(&id)
            .ok_or(ModelError::InvalidTransition)?;
        if record.outcome != LaunchOutcome::Pending
            || record.agent_id.is_some_and(|saved| saved != agent)
            || record.provider != a.provider
            || record.room_id != a.room_id
            || self.current_launch_attempt(agent).is_some_and(|current| {
                current.id != id && current.outcome == LaunchOutcome::Pending
            })
        {
            return Err(ModelError::InvalidTransition);
        }
        if record.agent_id != Some(agent) {
            Arc::make_mut(&mut self.launch_attempts)
                .get_mut(&id)
                .ok_or(ModelError::InvalidTransition)?
                .agent_id = Some(agent);
        }
        self.agents
            .get_mut(&agent)
            .ok_or(ModelError::UnknownAgent(agent))?
            .launch_attempt_id = Some(id);
        Ok(())
    }

    /// Fill an identity once, first launch then terminal. A previous terminal's
    /// observation can never rebind this record or overwrite a settled outcome.
    pub(crate) fn bind_launch_identity(
        &mut self,
        id: u64,
        identity: &AgentRuntimeIdentity,
    ) -> bool {
        let Some(record) = self.launch_attempts.get(&id) else {
            return false;
        };
        if record.outcome != LaunchOutcome::Pending
            || identity.launch_id.is_none()
            || record
                .launch_id
                .as_ref()
                .is_some_and(|saved| Some(saved) != identity.launch_id.as_ref())
            || record
                .terminal_id
                .as_ref()
                .is_some_and(|saved| Some(saved) != identity.terminal_id.as_ref())
            || record.launch_id == identity.launch_id && record.terminal_id == identity.terminal_id
        {
            return false;
        }
        let Some(record) = Arc::make_mut(&mut self.launch_attempts).get_mut(&id) else {
            return false;
        };
        record.launch_id = identity.launch_id.clone();
        record.terminal_id = identity.terminal_id.clone();
        true
    }

    pub(crate) fn settle_launch_attempt(
        &mut self,
        id: u64,
        identity: &AgentRuntimeIdentity,
        outcome: LaunchOutcome,
        cause: Option<&str>,
        at_ms: u64,
    ) -> bool {
        let Some(record) = self.launch_attempts.get(&id) else {
            return false;
        };
        if record.outcome != LaunchOutcome::Pending
            || outcome == LaunchOutcome::Pending
            || record.launch_id != identity.launch_id
            || record.terminal_id != identity.terminal_id
            || outcome == LaunchOutcome::Usable
                && (record.launch_id.is_none() || record.terminal_id.is_none())
        {
            return false;
        }
        let Some(record) = Arc::make_mut(&mut self.launch_attempts).get_mut(&id) else {
            return false;
        };
        record.outcome = outcome;
        record.cause = if outcome == LaunchOutcome::Usable {
            None
        } else {
            cause.map(str::to_owned)
        };
        record.settled_at_ms = Some(at_ms);
        true
    }

    /// Match only live input records, never scan the permanent archive on a poll.
    pub(crate) fn launch_message_started(
        &self,
        agent: AgentId,
        launch: Option<&str>,
        floor: u64,
    ) -> bool {
        let Some(launch) = launch.filter(|_| floor != 0) else {
            return false;
        };
        self.requests.values().any(|r| {
            r.agent_id == agent
                && request_start_bound(self, r)
                && r.expected_launch_id.as_deref() == Some(launch)
                && r.submission_status_revision >= floor
        })
    }

    pub(crate) fn watch_launch_command(&mut self, agent: AgentId, started_at_ms: u64) {
        let Some(record) = self.current_launch_attempt(agent) else {
            return;
        };
        let deadline = started_at_ms
            .saturating_add(crate::protocol::api::AGENT_START_TIMEOUT.as_millis() as u64);
        if record.outcome != LaunchOutcome::Pending || record.deadline_at_ms == deadline {
            return;
        }
        let id = record.id;
        if let Some(record) = Arc::make_mut(&mut self.launch_attempts).get_mut(&id) {
            record.deadline_at_ms = deadline;
        }
    }

    pub(crate) fn launch_records_changed_from(&self, before: &Self) -> bool {
        !Arc::ptr_eq(&self.launch_attempts, &before.launch_attempts)
    }

    pub(crate) fn launch_attempts(&self) -> impl Iterator<Item = &LaunchAttempt> {
        self.launch_attempts.values()
    }
    pub(crate) fn launch_attempt(&self, id: u64) -> Option<&LaunchAttempt> {
        self.launch_attempts.get(&id)
    }
    pub(crate) fn current_launch_attempt(&self, agent: AgentId) -> Option<&LaunchAttempt> {
        self.launch_attempt(self.agents.get(&agent)?.launch_attempt_id?)
            .filter(|record| record.agent_id == Some(agent))
    }
    pub(crate) fn copy_launch_ledger_from(&mut self, state: &Self) {
        self.launch_attempts = Arc::clone(&state.launch_attempts);
        self.launch_tracking_started_at_ms = state.launch_tracking_started_at_ms;
    }
    pub(crate) fn launch_tracking_started_at_ms(&self) -> Option<u64> {
        self.launch_tracking_started_at_ms
    }

    pub(crate) fn cancel_pending_launch(
        &mut self,
        agent: AgentId,
        cause: &str,
        at_ms: u64,
    ) -> bool {
        let Some(record) = self.current_launch_attempt(agent) else {
            return false;
        };
        let id = record.id;
        let identity = AgentRuntimeIdentity {
            launch_id: record.launch_id.clone(),
            terminal_id: record.terminal_id.clone(),
            ..Default::default()
        };
        self.settle_launch_attempt(id, &identity, LaunchOutcome::Cancelled, Some(cause), at_ms)
    }
}

pub(super) mod records {
    use super::LaunchAttempt;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::{collections::BTreeMap, sync::Arc};
    type Records = BTreeMap<u64, LaunchAttempt>;

    pub(in crate::messaging::model) fn serialize<S: Serializer>(
        records: &Arc<Records>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        records.as_ref().serialize(s)
    }
    pub(in crate::messaging::model) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Arc<Records>, D::Error> {
        Records::deserialize(d).map(Arc::new)
    }
}
