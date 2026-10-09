//! Reconnect a Bus agent to the same provider conversation after PTY restore.
use super::{schema, BusState, RoomAgent};
use crate::messaging::identity;

mod capture;
#[cfg(test)]
pub(crate) use capture::load;
pub(crate) use capture::{for_native_resume, LaunchExtras, NativeResumeContext, NativeResumeFacts};

/// A Codex prompt screen can exist before Codex creates a conversation. Keep
/// that empty launch across restore; never turn an adopted session into a new one.
pub(crate) fn unstarted_codex_plan(
    facts: &NativeResumeFacts,
    argv: Option<&[String]>,
) -> Option<crate::agents::resume::catalog::AgentResumePlan> {
    if !facts.is_bus_owned()
        || facts.managed_agent.as_deref() != Some("codex")
        || facts.session.is_some()
    {
        return None;
    }
    let mut args = argv.unwrap_or_default().to_vec();
    if !args.is_empty() {
        if args.remove(0) != "codex" {
            return None;
        }
        args.retain(|arg| arg != "--no-daemon");
        crate::agents::providers::launch::validate_args(
            crate::agents::providers::ProviderKind::Codex,
            &args,
        )
        .ok()?;
    }
    args.insert(0, "codex".into());
    args.push("--no-daemon".into());
    Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "codex".into(),
        argv: args,
        dedupe_key: format!("bus-unstarted\0{}", facts.agent_name.as_deref()?),
    })
}

fn is_unstarted_codex(state: &BusState, agent: &RoomAgent) -> bool {
    agent.provider == super::Provider::Codex
        && agent.runtime_identity.session_id.is_none()
        && agent.current_request.is_none()
        && !state.requests().any(|request| {
            request.agent_id == agent.id && request.phase != super::RequestPhase::Queued
        })
}

pub(super) fn restored_agent<'a>(
    state: &BusState,
    agent: &RoomAgent,
    infos: &'a [schema::AgentInfo],
) -> Option<&'a schema::AgentInfo> {
    if agent.session_binding_invalidated {
        return None;
    }
    let session = agent
        .runtime_identity
        .session_id
        .as_deref()
        .filter(|s| !s.is_empty());
    if session.is_none() && !is_unstarted_codex(state, agent) {
        return None;
    }
    agent
        .runtime_identity
        .launch_id
        .as_deref()
        .filter(|s| !s.is_empty())?;
    let kind = identity::provider_kind(agent.provider).label();
    let name = identity::managed_name(agent.room_id, agent.id);
    let source = identity::managed_source(agent.provider);
    let mut matches = infos.iter().filter(|info| {
        info.name.as_deref() == Some(name.as_str())
            && info.agent.as_deref() == Some(kind)
            && match (session, info.agent_session.as_ref()) {
                (None, None) => true,
                (Some(session), Some(native)) => {
                    native.source == source
                        && native.agent == kind
                        && native.kind == schema::AgentSessionRefKind::Id
                        && native.value == session
                }
                _ => false,
            }
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
                    && session.is_some()
                    && other.runtime_identity.session_id.as_deref() == session))
    }) {
        return None;
    }
    Some(candidate)
}
