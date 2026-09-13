//! Reconnect a Bus agent to the same provider conversation after PTY restore.
use super::*;

pub(super) fn restored_agent<'a>(
    state: &BusState,
    agent: &Agent,
    infos: &'a [schema::AgentInfo],
) -> Option<&'a schema::AgentInfo> {
    if agent.session_binding_invalidated {
        return None;
    }
    let session = agent
        .runtime_identity
        .session_id
        .as_deref()
        .filter(|s| !s.is_empty())?;
    agent
        .runtime_identity
        .launch_id
        .as_deref()
        .filter(|s| !s.is_empty())?;
    let kind = launch::provider_kind(agent.provider);
    let name = format!("bus-r{}-a{}", agent.room_id.0, agent.id.0);
    let source = format!("herdr:{kind}");
    let mut matches = infos.iter().filter(|info| {
        info.name.as_deref() == Some(name.as_str())
            && info.agent.as_deref() == Some(kind)
            && info.agent_session.as_ref().is_some_and(|native| {
                native.source == source
                    && native.agent == kind
                    && native.kind == crate::agent_resume::AgentSessionRefKind::Id
                    && native.value == session
            })
    });
    let candidate = matches.next()?;
    if matches.next().is_some() || candidate.terminal_id.is_empty() || candidate.pane_id.is_empty()
    {
        return None;
    }
    // Do not steal an already owned terminal or merge two Bus agents which
    // claim one conversation. Display names and reused pane IDs are not proof.
    if state.agents().any(|other| {
        other.id != agent.id
            && (other.runtime_identity.terminal_id.as_deref()
                == Some(candidate.terminal_id.as_str())
                || other.runtime_identity.pane_id.as_deref() == Some(candidate.pane_id.as_str())
                || (other.provider == agent.provider
                    && other.runtime_identity.session_id.as_deref() == Some(session)))
    }) {
        return None;
    }
    Some(candidate)
}
