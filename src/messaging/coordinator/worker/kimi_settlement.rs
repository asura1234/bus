//! Read Kimi terminal evidence before settling a request.
use super::{AgentId, BusState, Path, RuntimeStatus};

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
) -> Option<crate::messaging::model::RecordedTurnEnd> {
    use crate::agents::providers::kimi::{launch, wire};
    use crate::messaging::model::RecordedTurnEnd;
    let owner = state.agent(agent)?.clone();
    if owner.provider != crate::agents::providers::ProviderKind::Kimi
        || !matches!(
            owner.status,
            RuntimeStatus::Idle | RuntimeStatus::Unavailable
        )
    {
        return None;
    }
    let request = state.request(owner.current_request?)?.clone();
    let launch_id = request.expected_launch_id.as_deref()?;
    let session = request.provider_session_id.as_deref()?;
    let turn = request.provider_turn_id.as_deref()?;
    if !request.trusted_start_bound
        || owner.runtime_identity.launch_id.as_deref() != Some(launch_id)
        || owner.runtime_identity.session_id.as_deref() != Some(session)
    {
        return None;
    }
    let prompt = state.render_request_payload(request.id)?;
    let hooks = crate::agents::providers::hook_json::HookContext {
        binary: std::env::current_exe().ok()?,
        spool: data_dir.join("callbacks").join(launch_id),
        launch_id: launch_id.into(),
    };
    let home = launch::saved_home(&hooks).ok()?;
    match wire::resolve_outcome(&home, &owner.cwd, session, turn, &prompt).ok()?? {
        wire::Outcome::Final(text) => Some(RecordedTurnEnd::Final {
            turn_id: turn.into(),
            text,
            prompts: vec![prompt],
        }),
        wire::Outcome::NoReply => Some(RecordedTurnEnd::NoReply),
        wire::Outcome::Failed(message) => Some(RecordedTurnEnd::Error(message)),
    }
}
