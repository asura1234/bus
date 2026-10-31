//! Read Kimi terminal evidence before settling a request.
use super::{AgentId, BusState, Path, RuntimeStatus};

pub(super) fn state_provider_is_kimi(state: &BusState, agent: AgentId) -> bool {
    state
        .agent(agent)
        .is_some_and(|agent| agent.provider == crate::agents::providers::ProviderKind::Kimi)
}

/// A Kimi frame can be temporarily idle during tool work. Its own committed
/// terminal wire, tied to the launch, session, turn and prompt, is the witness.
pub(super) struct KimiEvidence {
    pub end: Option<crate::messaging::model::RecordedTurnEnd>,
    pub steer_turn: String,
    pub steered: Vec<(String, String)>,
}

pub(super) fn recorded_kimi_end(
    data_dir: &Path,
    state: &mut BusState,
    agent: AgentId,
) -> Result<KimiEvidence, String> {
    use crate::agents::providers::kimi::{launch, wire};
    use crate::messaging::model::RecordedTurnEnd;
    let empty = KimiEvidence {
        end: None,
        steer_turn: String::new(),
        steered: Vec::new(),
    };
    let Some(owner) = state.agent(agent).cloned() else {
        return Ok(empty);
    };
    if owner.provider != crate::agents::providers::ProviderKind::Kimi
        || !matches!(
            owner.status,
            RuntimeStatus::Idle | RuntimeStatus::Unavailable
        )
    {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: String::new(),
            steered: Vec::new(),
        });
    }
    let Some(current) = owner.current_request else {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: String::new(),
            steered: Vec::new(),
        });
    };
    let Some(request) = state.request(current).cloned() else {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: String::new(),
            steered: Vec::new(),
        });
    };
    let Some(launch_id) = request.expected_launch_id.as_deref() else {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: String::new(),
            steered: Vec::new(),
        });
    };
    let Some(session) = request.provider_session_id.as_deref() else {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: String::new(),
            steered: Vec::new(),
        });
    };
    let Some(turn) = request.provider_turn_id.as_deref() else {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: String::new(),
            steered: Vec::new(),
        });
    };
    if !request.trusted_start_bound
        || owner.runtime_identity.launch_id.as_deref() != Some(launch_id)
        || owner.runtime_identity.session_id.as_deref() != Some(session)
    {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: turn.to_owned(),
            steered: Vec::new(),
        });
    }
    let Some(prompt) = state.render_request_payload(request.id) else {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: turn.to_owned(),
            steered: Vec::new(),
        });
    };
    let Ok(executable) = std::env::current_exe() else {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: turn.to_owned(),
            steered: Vec::new(),
        });
    };
    let hooks = crate::agents::providers::hook_json::HookContext {
        binary: executable,
        spool: data_dir.join("callbacks").join(launch_id),
        launch_id: launch_id.into(),
    };
    let home = launch::saved_home(&hooks)?;
    let Some(capture) = wire::resolve_capture(&home, &owner.cwd, session, turn, &prompt)? else {
        return Ok(KimiEvidence {
            end: None,
            steer_turn: turn.to_owned(),
            steered: Vec::new(),
        });
    };
    let end = if let Some((continuation, text)) = capture.continuation {
        Some(RecordedTurnEnd::Final {
            turn_id: continuation.to_string(),
            text,
            prompts: vec![prompt],
        })
    } else {
        match capture.lead {
            Some(wire::Outcome::Final(text)) => Some(RecordedTurnEnd::Final {
                turn_id: turn.to_owned(),
                text,
                prompts: vec![prompt],
            }),
            Some(wire::Outcome::NoReply) => Some(RecordedTurnEnd::NoReply),
            Some(wire::Outcome::Failed(message)) => Some(RecordedTurnEnd::Error(message)),
            None => None,
        }
    };
    Ok(KimiEvidence {
        end,
        steer_turn: turn.to_owned(),
        steered: capture.steered,
    })
}
