//! Reconnect a Bus agent to the same provider conversation after PTY restore.
use super::{schema, BusState, RoomAgent};
use crate::messaging::identity;

mod capture;
#[cfg(all(test, unix))]
#[path = "resume/tests/kept_owner_fixture_test.rs"]
pub(crate) mod kept_owner_fixture;
#[cfg(test)]
pub(crate) use capture::load;
pub(crate) use capture::{
    for_native_resume, revalidate, LaunchExtras, NativeResumeContext, NativeResumeFacts,
    RETIRED_OWNER,
};

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
        "claude-ds" => ProviderKind::ClaudeDs,
        "claude-argon" => ProviderKind::ClaudeArgon,
        "claude-gpt" => ProviderKind::ClaudeGpt,
        "cursor" => ProviderKind::Cursor,
        "grok" => ProviderKind::Grok,
        "agy" => ProviderKind::Agy,
        _ => return None,
    };
    let executable = launch::executable(provider);
    let mut args = argv.unwrap_or_default().to_vec();
    if !args.is_empty() {
        if provider == ProviderKind::Grok {
            crate::agents::providers::grok::launch::validate_unstarted_argv(&args).ok()?;
        } else if provider == ProviderKind::Agy {
            crate::agents::providers::agy::launch::validate_unstarted_argv(&args).ok()?;
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
            | super::Provider::ClaudeDs
            | super::Provider::ClaudeArgon
            | super::Provider::ClaudeGpt
            | super::Provider::Cursor
            | super::Provider::Grok
            | super::Provider::Kimi
            | super::Provider::Agy
    ) && agent.runtime_identity.session_id.is_none()
        && no_request_reached_provider(state, agent)
}

fn no_request_reached_provider(state: &BusState, agent: &RoomAgent) -> bool {
    agent.current_request.is_none()
        && !state.requests().any(|request| {
            // A closed startup-failed send never reached provider input either.
            request.agent_id == agent.id
                && request.phase != super::RequestPhase::Queued
                && !request.failed_during_startup()
        })
}

fn claude_startup_only(state: &BusState, agent: &RoomAgent) -> bool {
    agent.provider.uses_claude_harness()
        && !agent.claude_session_has_turn
        && no_request_reached_provider(state, agent)
        && agent
            .runtime_identity
            .session_id
            .as_deref()
            .is_some_and(crate::agents::providers::launch::is_session_uuid)
}

impl super::Worker {
    pub(super) fn reconcile_fresh_claude_startup(
        &mut self,
        state: &mut BusState,
        agent: &RoomAgent,
        parsed: &crate::agents::providers::spool::Parsed,
        session: &str,
    ) -> Result<Option<bool>, String> {
        use crate::agents::providers::spool::Parsed;
        if !matches!(parsed, Parsed::Session { source, .. } if source.as_deref() == Some("startup"))
            || !claude_startup_only(state, agent)
        {
            return Ok(Some(false));
        }
        let infos = match self
            .transport
            .request(super::Method::AgentList(schema::EmptyParams {}))
        {
            Ok(super::ResponseResult::AgentList { agents }) => agents,
            _ => return Ok(None),
        };
        let Some(info) = restored_agent(state, agent, &infos).filter(|info| {
            native_attempt(agent, info).is_some_and(|attempt| attempt.replaced_session_id.is_some())
                && info
                    .agent_session
                    .as_ref()
                    .is_none_or(|native| native.value == session)
        }) else {
            return Ok(Some(false));
        };
        super::poll::rebind_agent(state, agent, info, self.serving_generation)?;
        super::startup::watch_native_resume(state, agent, info)?;
        Ok(Some(true))
    }
}

/// Correlate native-owned restore evidence, never screen text or a reused pane.
pub(super) fn native_attempt<'a>(
    agent: &RoomAgent,
    info: &'a schema::AgentInfo,
) -> Option<&'a schema::NativeResumeInfo> {
    let attempt = info.native_resume.as_ref()?;
    let kind = identity::provider_kind(agent.provider).label();
    let official_session = |session: &schema::AgentSessionInfo| {
        session.agent == kind
            && session.source == identity::managed_source(agent.provider)
            && session.kind == schema::AgentSessionRefKind::Id
    };
    let matches_session = if let Some(previous) = attempt.replaced_session_id.as_ref() {
        agent.provider.uses_claude_harness()
            && attempt.session_id.is_none()
            && (agent.runtime_identity.session_id.as_ref() == Some(previous)
                || agent.runtime_identity.session_id.is_none()
                    && agent.superseded_session_id.as_ref() == Some(previous))
            && info
                .agent_session
                .as_ref()
                .is_none_or(|session| official_session(session) && session.value != *previous)
    } else {
        attempt.session_id == agent.runtime_identity.session_id
            && info.agent_session.as_ref().is_none_or(|session| {
                official_session(session) && Some(&session.value) == attempt.session_id.as_ref()
            })
    };
    (attempt.name == identity::managed_name(agent.room_id, agent.id)
        && agent.provider.observes_label(&attempt.agent)
        && matches_session
        && info
            .agent
            .as_deref()
            .is_none_or(|actual| agent.provider.observes_label(actual)))
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
                && (info
                    .agent
                    .as_deref()
                    .is_some_and(|actual| agent.provider.observes_label(actual))
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
