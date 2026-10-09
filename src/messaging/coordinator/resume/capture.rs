//! Validate saved ownership before reconstructing a provider's capture extras.

use std::path::{Path, PathBuf};

use crate::agents::{
    providers::{hook_json::HookContext, ProviderKind},
    resume::catalog::{AgentResumePlan, AgentSessionRefKind, PersistedAgentSession},
};
use crate::messaging::{identity, storage::state_store::JsonStore};

mod ownership;

/// An immutable snapshot extracted by the server. No PTY/state handle crosses
/// this boundary, and the snapshot alone does not attest ownership.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct NativeResumeFacts {
    pub(crate) agent_name: Option<String>,
    pub(crate) managed_agent: Option<String>,
    pub(crate) session: Option<PersistedAgentSession>,
}

impl NativeResumeFacts {
    pub(crate) fn is_bus_owned(&self) -> bool {
        self.agent_name
            .as_deref()
            .is_some_and(|name| name.starts_with("bus-r"))
    }
}

/// Caller-captured native server context; providers do not read ambient room,
/// session or CLI configuration themselves.
#[derive(Clone, Debug)]
pub(crate) struct NativeResumeContext {
    pub(crate) root: Option<PathBuf>,
    pub(crate) session_name: Option<String>,
    pub(crate) config_root: PathBuf,
    pub(crate) project: PathBuf,
    pub(crate) binary: PathBuf,
}

impl NativeResumeContext {
    pub(crate) fn is_bus_server(&self) -> bool {
        self.root
            .as_ref()
            .is_some_and(|root| root.is_absolute() && self.config_root == root.join("herdr-config"))
            && self.session_name.as_deref() == Some(super::super::DEFAULT_SESSION)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LaunchExtras {
    pub(crate) env: Vec<(String, String)>,
    pub(crate) args: Vec<String>,
    /// Use the conversation Bus hooks bound when the terminal kept a stale one.
    pub(crate) session: Option<PersistedAgentSession>,
}

pub(crate) fn for_native_resume(
    context: &NativeResumeContext,
    facts: &NativeResumeFacts,
    plan: &AgentResumePlan,
) -> Result<LaunchExtras, String> {
    // Reserved names or inherited data-root variables do not opt another
    // native server into Bus lifecycle ownership.
    if !facts.is_bus_owned() || !context.is_bus_server() {
        return Ok(LaunchExtras::default());
    }
    load(context, facts, plan)
}

pub(crate) fn load(
    context: &NativeResumeContext,
    facts: &NativeResumeFacts,
    plan: &AgentResumePlan,
) -> Result<LaunchExtras, String> {
    if !facts.is_bus_owned() {
        return Ok(LaunchExtras::default());
    }
    let root = context
        .root
        .as_deref()
        .filter(|root| root.is_absolute())
        .ok_or("Bus resume data root unavailable")?;
    if !context.is_bus_server() {
        return Err("Bus resume context belongs to another server session".into());
    }
    let state = JsonStore::new(root.join("state.json"))
        .load()
        .map_err(|error| {
            let (stage, raw_os_error, _) = error.diagnostic();
            tracing::warn!(event = "bus.resume.state_unreadable", stage, raw_os_error = ?raw_os_error,
                "Bus resume state load failed");
            "Bus resume state is unreadable"
        })?
        .ok_or("Bus resume state is missing")?;
    let (agent, corrected) = if facts.session.is_none() {
        if super::unstarted_codex_plan(facts, Some(&plan.argv)).as_ref() != Some(plan) {
            return Err("Bus resume has no saved provider session".into());
        }
        (ownership::unstarted_owner(&state, facts)?, None)
    } else {
        ownership::resume_owner(&state, facts, saved_session(facts, plan)?)?
    };
    if agent.deletion_pending
        || state
            .room(agent.room_id)
            .is_none_or(|room| room.deletion_pending)
        || agent.session_binding_invalidated
    {
        return Err("Bus resume is suspended by deletion or invalidated identity".into());
    }
    let hooks = ownership::verified_capture(root, agent, &context.binary)?;
    if facts.session.is_none()
        && crate::agents::providers::launch::reserved_session(&hooks.spool).is_some()
    {
        return Err("Bus resume cannot replace an adopted session with an empty launch".into());
    }
    let (path, mut args, rebound) = capture_args(agent.provider, &hooks, &context.project)?;
    if rebound {
        tracing::info!(event = "bus.resume.hooks_rebound", agent_id = agent.id.0,
            path = %path.display(), "Bus hooks moved back to this executable for resume");
    }
    // Prompt policy stays with messaging; provider-specific argv stays below it.
    args.extend(crate::messaging::orchestration::resume_prompt_args(
        agent.provider,
        &hooks.spool,
    )?);
    let mut env = hooks.env();
    let env = ["BUS_LAUNCH_ID", "BUS_CALLBACK_DIR"]
        .map(|key| {
            (
                key.to_owned(),
                env.remove(key)
                    .expect("HookContext sets both capture variables"),
            )
        })
        .into();
    Ok(LaunchExtras {
        env,
        args,
        session: corrected,
    })
}

fn saved_session<'a>(
    facts: &'a NativeResumeFacts,
    plan: &AgentResumePlan,
) -> Result<&'a PersistedAgentSession, String> {
    let session = facts
        .session
        .as_ref()
        .ok_or("Bus resume has no saved provider session")?;
    if session.session_ref.kind != AgentSessionRefKind::Id
        || facts.managed_agent.as_deref() != Some(session.agent.as_str())
        || crate::agents::resume::catalog::plan(
            &session.source,
            &session.agent,
            &session.session_ref,
        )
        .as_ref()
            != Some(plan)
    {
        return Err("Bus resume plan does not match its official saved provider session".into());
    }
    Ok(session)
}

fn capture_args(
    provider: crate::messaging::model::Provider,
    hooks: &HookContext,
    project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    use crate::agents::providers::{claude_code, codex, cursor};
    match identity::provider_kind(provider) {
        ProviderKind::ClaudeCode => claude_code::resume::capture_args(hooks, project),
        ProviderKind::Codex => codex::resume::capture_args(hooks, project),
        ProviderKind::Cursor => cursor::resume::capture_args(hooks, project),
    }
}
