//! Reconnect a Bus agent to the same provider conversation after PTY restore.
use super::{schema, BusState, RoomAgent};
use crate::messaging::identity;

mod capture;
#[cfg(test)]
pub(crate) use capture::load;
pub(crate) use capture::{for_native_resume, LaunchExtras, NativeResumeContext, NativeResumeFacts};

/// Sessionless managed providers keep their empty launch across restore.
/// Snapshot argv must pass that provider's launch-argument check, so Kimi
/// attached `-S<session>` and `-p<text>` cannot become a fresh plan.
/// Never turn an adopted session into a new one.
pub(crate) fn unstarted_plan(
    facts: &NativeResumeFacts,
    argv: Option<&[String]>,
) -> Option<crate::agents::resume::catalog::AgentResumePlan> {
    if !facts.is_bus_owned() || facts.session.is_some() {
        return None;
    }
    let kind = facts.managed_agent.as_deref()?;
    if !matches!(kind, "codex" | "kimi") {
        return unstarted_provider_plan(facts, argv);
    }
    let mut args = argv.unwrap_or_default().to_vec();
    if !args.is_empty() {
        if kind == "kimi" {
            if args.first().map(String::as_str) == Some("/usr/bin/env") {
                if args.get(1).map(String::as_str) != Some("KIMI_DISABLE_OAUTH_LOCK=0")
                    || args
                        .get(2)
                        .is_none_or(|arg| !arg.starts_with("KIMI_CODE_HOME="))
                {
                    return None;
                }
                args.drain(0..3);
                // Older Bus snapshots lack the generated update switch.
                if args.first().map(String::as_str) == Some("KIMI_CODE_NO_AUTO_UPDATE=1") {
                    args.remove(0);
                }
            }
            let executable = args.first()?;
            if std::path::Path::new(executable)
                .file_name()
                .and_then(|name| name.to_str())
                != Some("kimi")
            {
                return None;
            }
            args.remove(0);
            if crate::agents::providers::kimi::launch::validate_owned_launch(&args).is_err() {
                return None;
            }
            return Some(crate::agents::resume::catalog::AgentResumePlan {
                agent: kind.into(),
                argv: vec![kind.into()],
                dedupe_key: format!("bus-unstarted\0{}", facts.agent_name.as_deref()?),
            });
        }
        if args.remove(0) != kind {
            return None;
        }
        args.retain(|arg| arg != "--no-daemon");
        // A native session selector must never become a fresh empty launch.
        if crate::agents::providers::launch::argument_positions(
            crate::agents::providers::ProviderKind::Codex,
            &args,
        )
        .into_iter()
        .any(|index| !args[index].starts_with('-'))
        {
            return None;
        }
        crate::agents::providers::launch::validate_args(
            crate::agents::providers::ProviderKind::Codex,
            &args,
        )
        .ok()?;
    }
    // Snapshot argv only verifies an interactive, unstarted launch. User
    // options have one owner: the launch record loaded by capture::load.
    Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: kind.into(),
        argv: vec![kind.into()],
        dedupe_key: format!("bus-unstarted\0{}", facts.agent_name.as_deref()?),
    })
}

/// A timed-out fresh provider can restart without inventing a conversation.
/// Snapshot argv only validates the plan; durable capture owns launch options.
fn unstarted_provider_plan(
    facts: &NativeResumeFacts,
    argv: Option<&[String]>,
) -> Option<crate::agents::resume::catalog::AgentResumePlan> {
    use crate::agents::providers::{launch, ProviderKind};
    let provider = match facts.managed_agent.as_deref()? {
        "claude" => ProviderKind::ClaudeCode,
        "cursor" => ProviderKind::Cursor,
        "grok" => ProviderKind::Grok,
        _ => return None,
    };
    let executable = launch::executable(provider);
    let mut args = argv.unwrap_or_default().to_vec();
    if !args.is_empty() {
        if provider == ProviderKind::Grok {
            crate::agents::providers::grok::launch::validate_unstarted_argv(&args).ok()?;
        } else {
            if std::path::Path::new(&args.remove(0))
                .file_name()?
                .to_str()?
                != executable
            {
                return None;
            }
            launch::validate_args(provider, &args).ok()?;
            // Removing a selector/prompt must not invent a fresh conversation.
            if args.iter().any(|arg| arg == "--")
                || launch::resumable_options(provider, &args) != args
                || launch::argument_positions(provider, &args)
                    .into_iter()
                    .any(|index| !args[index].starts_with('-'))
            {
                return None;
            }
        }
    }
    Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: provider.label().into(),
        argv: vec![executable.into()],
        dedupe_key: format!("bus-unstarted\0{}", facts.agent_name.as_deref()?),
    })
}

/// Sessionless contract: no provider session yet, and no
/// request has reached provider input.
fn is_unstarted(state: &BusState, agent: &RoomAgent) -> bool {
    matches!(
        agent.provider,
        super::Provider::Codex
            | super::Provider::ClaudeCode
            | super::Provider::Cursor
            | super::Provider::Grok
            | super::Provider::Kimi
    ) && agent.runtime_identity.session_id.is_none()
        && agent.current_request.is_none()
        && !state.requests().any(|request| {
            // A closed startup-failed send never reached provider input either.
            request.agent_id == agent.id
                && request.phase != super::RequestPhase::Queued
                && request.failure_reason.as_deref()
                    != Some(crate::messaging::model::AGENT_STARTUP_FAILED)
        })
}

/// Correlate native-owned restore evidence, never screen text or a reused pane.
pub(super) fn native_attempt<'a>(
    agent: &RoomAgent,
    info: &'a schema::AgentInfo,
) -> Option<&'a schema::NativeResumeInfo> {
    let attempt = info.native_resume.as_ref()?;
    let kind = identity::provider_kind(agent.provider).label();
    (attempt.name == identity::managed_name(agent.room_id, agent.id)
        && attempt.agent == kind
        && attempt.session_id == agent.runtime_identity.session_id
        && info.agent_session.as_ref().is_none_or(|session| {
            session.agent == kind
                && session.source == identity::managed_source(agent.provider)
                && session.kind == schema::AgentSessionRefKind::Id
                && Some(&session.value) == attempt.session_id.as_ref()
        })
        && info.agent.as_deref().is_none_or(|actual| actual == kind))
    .then_some(attempt)
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
    if session.is_none() && !is_unstarted(state, agent) {
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
        native_attempt(agent, info).is_some()
            || (info.name.as_deref() == Some(name.as_str())
                && (info.agent.as_deref() == Some(kind)
                    || info.agent.is_none() && info.launch_pending)
                && match (session, info.agent_session.as_ref()) {
                    (None, None) => true,
                    (Some(session), Some(native)) => {
                        native.source == source
                            && native.agent == kind
                            && native.kind == schema::AgentSessionRefKind::Id
                            && native.value == session
                    }
                    _ => false,
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
                    && session.is_some()
                    && other.runtime_identity.session_id.as_deref() == session))
    }) {
        return None;
    }
    Some(candidate)
}
