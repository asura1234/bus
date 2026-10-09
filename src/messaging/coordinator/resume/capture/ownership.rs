//! Current session, owner and durable capture attestations remain messaging policy.
use std::path::Path;

use super::NativeResumeFacts;
use crate::agents::providers::{
    hook_json::{read_resume_json, HookContext},
    spool::Manifest,
};
use crate::messaging::{identity, model::RoomAgent};

pub(super) fn unstarted_owner<'a>(
    state: &'a crate::messaging::model::BusState,
    facts: &NativeResumeFacts,
) -> Result<&'a RoomAgent, String> {
    let mut owners = state.agents().filter(|agent| {
        facts.agent_name.as_deref()
            == Some(identity::managed_name(agent.room_id, agent.id).as_str())
            && super::super::is_unstarted_codex(state, agent)
    });
    let agent = owners
        .next()
        .ok_or("Bus resume has no matching unstarted Codex owner")?;
    if owners.next().is_some() {
        return Err("Bus resume unstarted ownership is ambiguous".into());
    }
    Ok(agent)
}

pub(super) fn verified_capture(
    root: &Path,
    agent: &RoomAgent,
    binary: &Path,
) -> Result<HookContext, String> {
    let launch = agent
        .runtime_identity
        .launch_id
        .as_deref()
        .filter(|launch| {
            !launch.is_empty()
                && launch
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
        .ok_or("Bus resume launch identity is missing or invalid")?;
    let spool = root.join("callbacks").join(launch);
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "Bus resume root unavailable")?;
    if spool
        .canonicalize()
        .map_err(|_| "Bus resume callback directory missing")?
        != canonical_root.join("callbacks").join(launch)
    {
        return Err("Bus resume callback directory escaped its saved root".into());
    }
    let manifest: Manifest =
        serde_json::from_value(read_resume_json(&spool.join("manifest.json"))?)
            .map_err(|_| "Bus resume callback manifest is invalid")?;
    if manifest.routing_key.0 != agent.id.0
        || manifest.provider != identity::provider_kind(agent.provider)
        || manifest.launch_id != launch
    {
        return Err("Bus resume callback manifest does not match its saved owner".into());
    }
    Ok(HookContext {
        binary: binary.to_path_buf(),
        spool,
        launch_id: launch.into(),
    })
}

pub(super) fn resume_owner<'a>(
    state: &'a crate::messaging::model::BusState,
    facts: &NativeResumeFacts,
    session: &crate::agents::resume::catalog::PersistedAgentSession,
) -> Result<
    (
        &'a RoomAgent,
        Option<crate::agents::resume::catalog::PersistedAgentSession>,
    ),
    String,
> {
    let owns = |agent: &&RoomAgent, value: &str| {
        identity::provider_kind(agent.provider).label() == session.agent
            && agent.runtime_identity.session_id.as_deref() == Some(value)
    };
    let mut candidates = state
        .agents()
        .filter(|agent| owns(agent, &session.session_ref.value));
    let mut corrected = None;
    let agent = match candidates.next() {
        Some(agent) => {
            if candidates.next().is_some()
                || facts.agent_name.as_deref()
                    != Some(identity::managed_name(agent.room_id, agent.id).as_str())
            {
                return Err("Bus resume ownership is ambiguous or mismatched".into());
            }
            agent
        }
        // The terminal kept a conversation Bus moved away from, as an older
        // server did for a Cursor new chat after `agent clear`. The agent
        // this terminal was launched for, by name and provider, resumes the
        // conversation its own hooks bound, never one another agent owns.
        None => {
            let mut named = state.agents().filter(|agent| {
                identity::provider_kind(agent.provider).label() == session.agent
                    && facts.agent_name.as_deref()
                        == Some(identity::managed_name(agent.room_id, agent.id).as_str())
            });
            let agent = named
                .next()
                .ok_or("Bus resume has no matching saved owner")?;
            let bound = agent
                .runtime_identity
                .session_id
                .as_deref()
                .ok_or("Bus resume has no matching saved owner")?;
            if named.next().is_some()
                || state.agents().filter(|other| owns(other, bound)).count() != 1
            {
                return Err("Bus resume ownership is ambiguous or mismatched".into());
            }
            let session_ref = crate::agents::resume::catalog::AgentSessionRef::id(bound)
                .ok_or("Bus resume session is not a provider session ID")?;
            corrected = Some(crate::agents::resume::catalog::PersistedAgentSession {
                source: session.source.clone(),
                agent: session.agent.clone(),
                session_ref,
            });
            agent
        }
    };
    Ok((agent, corrected))
}
