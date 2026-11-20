//! Read AGY transcript evidence before settling an unclaimed or unbound request.
use crate::agents::providers::agy::completion::{
    find_turn_for_prompt, read_turn_completion, TurnResolution,
};
use crate::messaging::model::{
    AgentId, BusState, Provider, RecordedTurnEnd, Request, RequestPhase, RoomAgent,
};
use std::path::{Path, PathBuf};

pub(super) fn state_provider_is_agy(state: &BusState, agent: AgentId) -> bool {
    state
        .agent(agent)
        .is_some_and(|owner| owner.provider == Provider::Agy)
}

pub(super) fn agy_transcript_for_agent(
    cached: Option<&(String, PathBuf)>,
    agent: &RoomAgent,
) -> Option<PathBuf> {
    let session = agent
        .runtime_identity
        .session_id
        .as_deref()
        .filter(|s| !s.is_empty())?;
    if let Some((cached_session, path)) = cached {
        if cached_session == session && path.is_file() {
            return Some(path.clone());
        }
    }
    if session.contains('/') || session.contains('\\') || session == "." || session == ".." {
        return None;
    }
    let candidate = std::env::home_dir()?
        .join(".gemini/antigravity-cli/brain")
        .join(session)
        .join(".system_generated/logs/transcript_full.jsonl");
    candidate.is_file().then_some(candidate)
}

/// Reconcile an AGY request (steered or standalone) directly from its committed
/// `transcript_full.jsonl` when no `PreInvocation(invocationNum == 0)` bound it.
pub(super) fn recorded_agy_end(
    state: &BusState,
    transcript: &Path,
    request: &Request,
) -> Option<RecordedTurnEnd> {
    if !matches!(
        request.phase,
        RequestPhase::Submitting | RequestPhase::Active
    ) {
        return None;
    }
    let launch = request.expected_launch_id.as_deref()?;
    let session = request.provider_session_id.as_deref().or_else(|| {
        state
            .agent(request.agent_id)
            .and_then(|owner| owner.runtime_identity.session_id.as_deref())
    });
    let turn_id = match &request.provider_turn_id {
        Some(turn) => turn.clone(),
        None => match find_turn_for_prompt(
            transcript,
            |prompt| request.matches_callback_payload(prompt, Provider::Agy),
            |turn| state.is_consumed_provider_turn(launch, session, Some(turn)),
        ) {
            Ok(Some(turn)) => turn,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(
                    event = "bus.agy.transcript_unreadable",
                    request_id = request.id.0,
                    agent_id = request.agent_id.0,
                    %error,
                    "AGY transcript could not be scanned for turn reconciliation"
                );
                return None;
            }
        },
    };
    match read_turn_completion(transcript, &turn_id) {
        Ok(TurnResolution::Final {
            turn_id,
            text,
            prompts,
        }) => Some(RecordedTurnEnd::Final {
            turn_id,
            text,
            prompts,
        }),
        Ok(TurnResolution::Pending) => None,
        Err(error) => {
            tracing::warn!(
                event = "bus.agy.transcript_unreadable",
                request_id = request.id.0,
                agent_id = request.agent_id.0,
                turn_id = %turn_id,
                %error,
                "AGY transcript could not be read for turn completion"
            );
            None
        }
    }
}
