//! Read Kimi terminal evidence before settling a request.
use super::{AgentId, BusState, Path, RequestId, RuntimeStatus};

pub(super) fn state_provider_is_kimi(state: &BusState, agent: AgentId) -> bool {
    state
        .agent(agent)
        .is_some_and(|agent| agent.provider == crate::agents::providers::ProviderKind::Kimi)
}

/// A Kimi frame can be temporarily idle during tool work. Its own committed
/// terminal wire, tied to the launch, session, turn and prompt, is the witness.
pub(super) fn recorded_kimi_end(
    data_dir: &Path,
    state: &mut BusState,
    agent: AgentId,
) -> Result<Option<crate::messaging::model::RecordedTurnEnd>, String> {
    recorded_kimi_request_end(data_dir, state, agent, None)
}

fn recorded_kimi_request_end(
    data_dir: &Path,
    state: &mut BusState,
    agent: AgentId,
    request: Option<RequestId>,
) -> Result<Option<crate::messaging::model::RecordedTurnEnd>, String> {
    match committed_kimi_end(data_dir, state, agent, request) {
        // A busy provider may be appending a partial record. Read/parse errors
        // are not committed provider failures; keep their previous idle-only
        // diagnostic settlement without gating an owned native error commit.
        Err(_)
            if !state.agent(agent).is_some_and(|owner| {
                matches!(
                    owner.status,
                    RuntimeStatus::Idle | RuntimeStatus::Unavailable
                )
            }) =>
        {
            Ok(None)
        }
        result => result,
    }
}

fn committed_kimi_end(
    data_dir: &Path,
    state: &mut BusState,
    agent: AgentId,
    request_id: Option<RequestId>,
) -> Result<Option<crate::messaging::model::RecordedTurnEnd>, String> {
    use crate::agents::providers::kimi::{launch, wire};
    use crate::messaging::model::RecordedTurnEnd;
    let Some(owner) = state.agent(agent).cloned() else {
        return Ok(None);
    };
    if owner.provider != crate::agents::providers::ProviderKind::Kimi {
        return Ok(None);
    }
    let Some(current) = request_id.or(owner.current_request) else {
        return Ok(None);
    };
    let Some(request) = state.request(current).cloned() else {
        return Ok(None);
    };
    if request.agent_id != agent {
        return Ok(None);
    }
    let Some(launch_id) = request.expected_launch_id.as_deref() else {
        return Ok(None);
    };
    let Some(session) = request.provider_session_id.as_deref() else {
        return Ok(None);
    };
    let Some(turn) = request.provider_turn_id.as_deref() else {
        return Ok(None);
    };
    if !request.trusted_start_bound
        || owner.runtime_identity.launch_id.as_deref() != Some(launch_id)
        || owner.runtime_identity.session_id.as_deref() != Some(session)
    {
        return Ok(None);
    }
    let Some(prompt) = state.render_request_payload(request.id) else {
        return Ok(None);
    };
    let Ok(executable) = std::env::current_exe() else {
        return Ok(None);
    };
    let hooks = crate::agents::providers::hook_json::HookContext {
        binary: executable,
        spool: data_dir.join("callbacks").join(launch_id),
        launch_id: launch_id.into(),
    };
    let home = launch::saved_home(&hooks)?;
    Ok(
        match wire::resolve_turn(&home, &owner.cwd, session, turn, &prompt)? {
            Some(wire::CommittedTurn {
                outcome: wire::Outcome::Final(text),
                prompts,
            }) => Some(RecordedTurnEnd::Final {
                turn_id: turn.into(),
                text,
                prompts,
            }),
            Some(wire::CommittedTurn {
                outcome: wire::Outcome::NoReply,
                prompts,
            }) => {
                // A silent lead can still precede an unclaimed queued turn.
                // Keep its ownership anchor only when steering needs proof.
                let steering = state.group_members(current).iter().any(|id| {
                    state
                        .request(*id)
                        .is_some_and(|member| member.steered && !member.settled())
                });
                Some(if steering {
                    RecordedTurnEnd::Final {
                        turn_id: turn.into(),
                        text: String::new(),
                        prompts,
                    }
                } else {
                    RecordedTurnEnd::NoReply
                })
            }
            Some(wire::CommittedTurn {
                outcome: wire::Outcome::Failed(message),
                ..
            }) => Some(RecordedTurnEnd::Error(message)),
            None => None,
        },
    )
}

impl super::Worker {
    pub(in crate::messaging::coordinator) fn recorded_kimi_end(
        &self,
        state: &mut BusState,
        agent: AgentId,
    ) -> Result<Option<crate::messaging::model::RecordedTurnEnd>, String> {
        // Stop and a later Started can arrive while the new turn is Working.
        committed_kimi_end(&self.data_dir, state, agent, None)
    }

    pub(in crate::messaging::coordinator) fn settle_previous_kimi_requests(
        &self,
        state: &mut BusState,
        agent: AgentId,
        now_ms: u64,
    ) -> bool {
        if !state_provider_is_kimi(state, agent) {
            return false;
        }
        let current = state.agent(agent).and_then(|owner| owner.current_request);
        let previous: Vec<_> = state
            .unsettled_requests()
            .filter(|request| {
                request.agent_id == agent
                    && Some(request.id) != current
                    && request.group.is_none()
                    && request.trusted_start_bound
            })
            .map(|request| request.id)
            .collect();
        let mut changed = false;
        for id in previous {
            let recorded = recorded_kimi_request_end(&self.data_dir, state, agent, Some(id))
                .unwrap_or_else(|error| {
                    if state.record_error_episode(format!("kimi-evidence:{}:{error}", agent.0)) {
                        tracing::warn!(event = "bus.kimi.evidence_failed", agent_id = agent.0,
                            request_id = id.0, %error, "Kimi completion evidence failed; closing the request with an actionable error");
                    }
                    Some(crate::messaging::model::RecordedTurnEnd::Error(error))
                });
            changed |= state
                .settle_previous_kimi_request(agent, id, now_ms, recorded)
                .is_some();
        }
        changed
    }
}
