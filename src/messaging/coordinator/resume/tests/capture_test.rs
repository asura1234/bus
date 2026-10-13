use crate::agents::providers::{
    hook_json::{install_hooks, validate_resume_hooks, HookContract},
    spool::{self, Manifest, RoutingKey},
};
use crate::agents::resume::catalog::AgentResumePlan;
use crate::messaging::coordinator::resume::{
    for_native_resume, load, LaunchExtras, NativeResumeContext, NativeResumeFacts,
};
use crate::messaging::{identity, model::*, storage::state_store::JsonStore};
use serde_json::json;
use std::path::Path;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    project: PathBuf,
    binary: PathBuf,
    facts: NativeResumeFacts,
    plan: AgentResumePlan,
    state: BusState,
    provider: Provider,
    agent: AgentId,
}

impl Fixture {
    fn new(provider: Provider) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bus-resume-{}-{}-{}",
            std::process::id(),
            crate::utils::time::now_ns(),
            NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let binary = std::env::current_exe().unwrap();
        let mut state = BusState::new();
        let room = state.create_room("original room").unwrap();
        let agent = state
            .create_agent(room, "original agent", provider, project.clone(), None)
            .unwrap();
        state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some("owned-launch".into()),
                    terminal_id: Some("old-terminal".into()),
                    pane_id: Some("w1:p2".into()),
                    session_id: Some("original-session".into()),
                },
            )
            .unwrap();
        state.confirm_hook_setup(agent).unwrap();
        let label = identity::provider_kind(provider).label();
        let session = crate::agents::resume::catalog::PersistedAgentSession {
            source: format!("herdr:{label}"),
            agent: label.into(),
            session_ref: crate::agents::resume::catalog::AgentSessionRef::id("original-session")
                .unwrap(),
        };
        let plan = crate::agents::resume::catalog::plan(
            &session.source,
            &session.agent,
            &session.session_ref,
        )
        .unwrap();
        let facts = NativeResumeFacts {
            agent_name: Some("bus-r1-a2".into()),
            managed_agent: Some(label.into()),
            session: Some(session),
        };
        spool::initialize(
            &root.join("callbacks/owned-launch"),
            &Manifest {
                routing_key: RoutingKey(agent.0),
                provider: identity::provider_kind(provider),
                launch_id: "owned-launch".into(),
            },
        )
        .unwrap();
        let fixture = Self {
            root,
            project,
            binary,
            facts,
            plan,
            state,
            provider,
            agent,
        };
        fixture.save();
        fixture.write_hooks();
        fixture
    }

    fn save(&self) {
        JsonStore::new(self.root.join("state.json"))
            .save(&self.state)
            .unwrap();
    }

    fn hook_path(&self) -> PathBuf {
        match self.provider {
            Provider::ClaudeCode => self
                .root
                .join("callbacks/owned-launch/claude-settings.json"),
            Provider::Codex => self.project.join(".codex/hooks.json"),
            Provider::Cursor => self.project.join(".cursor/hooks.json"),
        }
    }

    fn write_hooks(&self) {
        let (adapter, events): (&str, &[&str]) = match self.provider {
            Provider::ClaudeCode => (
                "claude-hook",
                &["SessionStart", "UserPromptSubmit", "Stop", "StopFailure"],
            ),
            Provider::Codex => ("codex-hook", &["SessionStart", "UserPromptSubmit", "Stop"]),
            Provider::Cursor => (
                "cursor-hook",
                &[
                    "sessionStart",
                    "beforeSubmitPrompt",
                    "afterAgentResponse",
                    "stop",
                ],
            ),
        };
        let command = format!(
            "{} --bus-callback {adapter}",
            crate::platform::remote_reattach_program(&self.binary.to_string_lossy())
        );
        let mut hooks = serde_json::Map::new();
        for event in events {
            hooks.insert(
                (*event).into(),
                if self.provider == Provider::Cursor {
                    json!([{"command":command}])
                } else {
                    json!([{"hooks":[{"type":"command","command":command,"timeout":5}]}])
                },
            );
        }
        let path = self.hook_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"version":1,"hooks":hooks})).unwrap(),
        )
        .unwrap();
    }

    fn load(&self) -> Result<LaunchExtras, String> {
        load(&self.context(), &self.facts, &self.plan)
    }

    fn context(&self) -> NativeResumeContext {
        NativeResumeContext {
            root: Some(self.root.clone()),
            session_name: Some("bus".into()),
            config_root: self.root.join("herdr-config"),
            project: self.project.clone(),
            cwd: self.project.clone(),
            binary: self.binary.clone(),
        }
    }

    /// Callback capture env for the saved launch.
    fn expected_env(&self) -> Vec<(String, String)> {
        [
            ("BUS_LAUNCH_ID", "owned-launch".to_owned()),
            (
                "BUS_CALLBACK_DIR",
                self.root
                    .join("callbacks/owned-launch")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]
        .map(|(key, value)| (key.to_owned(), value))
        .into()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn unstarted_codex_resume_restores_capture_without_inventing_a_session() {
    let mut fixture = Fixture::new(Provider::Codex);
    let mut identity = fixture
        .state
        .agent(fixture.agent)
        .unwrap()
        .runtime_identity
        .clone();
    identity.session_id = None;
    fixture
        .state
        .set_agent_runtime_identity(fixture.agent, identity)
        .unwrap();
    fixture.facts.session = None;
    fixture.plan =
        crate::messaging::coordinator::resume::unstarted_codex_plan(&fixture.facts, None).unwrap();
    fixture.save();
    let extras = fixture.load().unwrap();
    assert_eq!(extras.env, fixture.expected_env());
    let argv: Vec<_> = fixture.plan.argv.iter().chain(extras.args.iter()).collect();
    assert_eq!(
        argv.iter()
            .filter(|arg| arg.as_str() == "--no-daemon")
            .count(),
        1
    );
    assert!(extras.session.is_none());
    assert!(fixture
        .state
        .agent(fixture.agent)
        .unwrap()
        .runtime_identity
        .session_id
        .is_none());

    std::fs::write(
        fixture.root.join("callbacks/owned-launch/adopted-session"),
        "reserved-session",
    )
    .unwrap();
    assert_eq!(
        fixture.load().unwrap_err(),
        "Bus resume cannot replace an adopted session with an empty launch"
    );
}

#[test]
fn unstarted_resume_rejects_a_bound_owner_and_noninteractive_argv() {
    let mut fixture = Fixture::new(Provider::Codex);
    fixture.facts.session = None;
    fixture.plan =
        crate::messaging::coordinator::resume::unstarted_codex_plan(&fixture.facts, None).unwrap();
    assert!(
        fixture.load().is_err(),
        "a bound owner must keep its conversation"
    );
    for argv in [
        vec!["codex", "resume", "previous"],
        vec!["codex", "exec", "prompt"],
        vec!["codex", "--config", "x=y"],
    ] {
        let argv: Vec<String> = argv.into_iter().map(str::to_owned).collect();
        assert!(crate::messaging::coordinator::resume::unstarted_codex_plan(
            &fixture.facts,
            Some(&argv),
        )
        .is_none());
    }
}

#[test]
fn bus_resume_restores_existing_capture_for_each_provider_without_rewriting_plan() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let fixture = Fixture::new(provider);
        let original = fixture.plan.clone();
        let extras = fixture.load().unwrap();
        assert_eq!(extras.env, fixture.expected_env());
        assert_eq!(
            extras.args,
            match provider {
                Provider::ClaudeCode => vec![
                    "--settings".to_owned(),
                    fixture.hook_path().to_string_lossy().into_owned(),
                ],
                // The shared app-server daemon runs hooks with the env of
                // whichever pane started it, misrouting this launch's callbacks.
                Provider::Codex => vec!["--no-daemon".to_owned()],
                Provider::Cursor => vec![],
            }
        );
        assert_eq!(fixture.plan, original);
    }
}

#[test]
fn bus_resume_delivers_an_orchestrator_prompt_again() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let fixture = Fixture::new(provider);
        let spool = fixture.root.join("callbacks/owned-launch");
        let path = crate::messaging::orchestration::write_prompt(&spool, "Run pr-1.").unwrap();
        let extras = fixture.load().unwrap();
        let expected = crate::messaging::orchestration::prompt_args(provider, &path, false)
            .unwrap()
            .unwrap_or_default();
        assert!(extras.args.ends_with(&expected), "{:?}", extras.args);
        assert_eq!(provider == Provider::Cursor, expected.is_empty());
    }
}

#[test]
fn bus_resume_rejects_unattested_or_suspended_ownership() {
    // A different saved session alone is not rejected: Bus resumes the
    // one its hooks bound (see the test above).
    for field in [
        "name",
        "provider",
        "plan",
        "deletion",
        "invalidated",
        "launch",
        "manifest",
        "hooks",
        "duplicate",
    ] {
        let mut fixture = Fixture::new(Provider::Codex);
        let mut value = serde_json::to_value(&fixture.state).unwrap();
        let key = fixture.agent.0.to_string();
        match field {
            "name" => fixture.facts.agent_name = Some("bus-r999-a2".into()),
            "provider" => value["agents"][&key]["provider"] = json!("cursor"),
            "plan" => fixture
                .plan
                .argv
                .push("--dangerously-bypass-approvals-and-sandbox".into()),
            "deletion" => value["agents"][&key]["deletion_pending"] = json!(true),
            "invalidated" => value["agents"][&key]["session_binding_invalidated"] = json!(true),
            "launch" => {
                value["agents"][&key]["runtime_identity"]["launch_id"] = json!("../owned-launch")
            }
            "manifest" => std::fs::write(
                fixture.root.join("callbacks/owned-launch/manifest.json"),
                b"{}",
            )
            .unwrap(),
            "hooks" => std::fs::remove_file(fixture.hook_path()).unwrap(),
            "duplicate" => {
                let mut other = value["agents"][&key].clone();
                other["id"] = json!(99);
                value["agents"]["99"] = other;
            }
            _ => unreachable!(),
        }
        fixture.state = serde_json::from_value(value).unwrap();
        fixture.save();
        assert!(fixture.load().is_err(), "{field}");
    }
}

#[test]
fn bus_resume_follows_the_conversation_bus_bound_when_the_terminal_kept_another() {
    let mut fixture = Fixture::new(Provider::Cursor);
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    let key = fixture.agent.0.to_string();
    value["agents"][&key]["runtime_identity"]["session_id"] = json!("new-chat");
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    let extras = fixture.load().unwrap();
    assert_eq!(extras.env, fixture.expected_env());
    let session = extras.session.unwrap();
    assert_eq!(session.session_ref.value, "new-chat");
    assert_eq!(session.agent, "cursor");

    // Another agent's conversation, or another agent's terminal, is never taken.
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    let mut other = value["agents"][&key].clone();
    other["id"] = json!(99);
    value["agents"]["99"] = other;
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    assert!(fixture.load().is_err());
    let fixture = Fixture::new(Provider::Cursor);
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    value["agents"][fixture.agent.0.to_string()]["runtime_identity"]["session_id"] =
        json!("new-chat");
    let mut fixture = fixture;
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    fixture.facts.agent_name = Some("bus-r1-a77".into());
    assert!(fixture.load().is_err());
}

#[test]
fn bus_resume_takes_back_hooks_another_bus_executable_rewrote() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let fixture = Fixture::new(provider);
        let other = fixture.root.join("throwaway/bus");
        install_hooks(
            &fixture.hook_path(),
            HookContract::for_provider(identity::provider_kind(provider)),
            &other,
        )
        .unwrap();
        let before = std::fs::read_to_string(fixture.hook_path()).unwrap();
        assert!(before.contains("throwaway"), "{before}");
        let extras = fixture
            .load()
            .unwrap_or_else(|error| panic!("{provider:?}: {error}"));
        assert_eq!(extras.env, fixture.expected_env());
        let after = std::fs::read_to_string(fixture.hook_path()).unwrap();
        assert!(!after.contains("throwaway"), "{after}");
        assert!(validate_resume_hooks(
            &fixture.hook_path(),
            HookContract::for_provider(crate::messaging::identity::provider_kind(provider)),
            &fixture.binary
        )
        .is_ok());
    }
    // A hook the user removed stays removed, and resume stays suspended.
    let fixture = Fixture::new(Provider::Codex);
    std::fs::write(fixture.hook_path(), br#"{"hooks":{}}"#).unwrap();
    assert!(fixture.load().is_err());
    assert_eq!(
        std::fs::read(fixture.hook_path()).unwrap(),
        br#"{"hooks":{}}"#
    );
}

#[test]
fn bus_resume_preserves_unconfirmed_delivery_gate_without_blocking_native_setup() {
    let mut fixture = Fixture::new(Provider::Cursor);
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    value["agents"][fixture.agent.0.to_string()]["hook_setup_confirmed"] = json!(false);
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    assert_eq!(fixture.load().unwrap().env, fixture.expected_env());
    let saved = JsonStore::new(fixture.root.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(!saved.agent(fixture.agent).unwrap().hook_setup_confirmed);
}

#[test]
fn bus_resume_is_scoped_to_bus_root_and_leaves_other_native_resumes_unchanged() {
    let mut fixture = Fixture::new(Provider::Codex);
    for (root, session, config) in [
        (None, Some("bus"), fixture.root.join("herdr-config")),
        (
            Some(fixture.root.as_path()),
            Some("other"),
            fixture.root.join("herdr-config"),
        ),
        (
            Some(fixture.root.as_path()),
            Some("bus"),
            fixture.root.join("other-config"),
        ),
    ] {
        let context = NativeResumeContext {
            root: root.map(Path::to_path_buf),
            session_name: session.map(str::to_owned),
            config_root: config,
            ..fixture.context()
        };
        assert!(load(&context, &fixture.facts, &fixture.plan).is_err());
    }
    fixture.facts.agent_name = Some("personal-agent".into());
    assert_eq!(
        load(
            &NativeResumeContext {
                root: None,
                session_name: None,
                config_root: Path::new("/unavailable").into(),
                ..fixture.context()
            },
            &fixture.facts,
            &fixture.plan
        )
        .unwrap(),
        LaunchExtras::default()
    );
}

#[test]
fn bus_resume_entry_leaves_non_bus_servers_alone_even_with_reserved_names() {
    let fixture = Fixture::new(Provider::Codex);
    for (root, session) in [(None, None), (Some(fixture.root.as_path()), Some("nested"))] {
        let context = NativeResumeContext {
            root: root.map(Path::to_path_buf),
            session_name: session.map(str::to_owned),
            ..fixture.context()
        };
        assert_eq!(
            for_native_resume(&context, &fixture.facts, &fixture.plan).unwrap(),
            LaunchExtras::default()
        );
    }
}

#[test]
fn bus_resume_rejects_fact_and_capture_mismatches_before_rewriting_hooks_or_state() {
    for field in [
        "source",
        "managed-provider",
        "missing-session",
        "session-kind",
        "missing-state",
        "unreadable-state",
        "missing-launch",
        "missing-spool",
        "manifest-owner",
        "manifest-provider",
        "manifest-launch",
        "room-deletion",
    ] {
        let mut fixture = Fixture::new(Provider::Codex);
        let state_path = fixture.root.join("state.json");
        let mut state = serde_json::to_value(&fixture.state).unwrap();
        let key = fixture.agent.0.to_string();
        match field {
            "source" => fixture.facts.session.as_mut().unwrap().source = "custom:codex".into(),
            "managed-provider" => fixture.facts.managed_agent = Some("cursor".into()),
            "missing-session" => fixture.facts.session = None,
            "session-kind" => {
                fixture.facts.session.as_mut().unwrap().session_ref.kind =
                    crate::agents::resume::catalog::AgentSessionRefKind::Path
            }
            "missing-state" => std::fs::remove_file(&state_path).unwrap(),
            "unreadable-state" => {
                std::fs::remove_file(&state_path).unwrap();
                std::fs::create_dir(&state_path).unwrap();
            }
            "missing-launch" => {
                state["agents"][&key]["runtime_identity"]["launch_id"] = json!(null);
                fixture.state = serde_json::from_value(state).unwrap();
                fixture.save();
            }
            "missing-spool" => {
                std::fs::remove_dir_all(fixture.root.join("callbacks/owned-launch")).unwrap()
            }
            "room-deletion" => {
                let room = fixture
                    .state
                    .agent(fixture.agent)
                    .unwrap()
                    .room_id
                    .0
                    .to_string();
                state["rooms"][room]["deletion_pending"] = json!(true);
                fixture.state = serde_json::from_value(state).unwrap();
                fixture.save();
            }
            _ => {
                let path = fixture.root.join("callbacks/owned-launch/manifest.json");
                let mut manifest: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                match field {
                    "manifest-owner" => manifest["agent_id"] = json!(99),
                    "manifest-provider" => manifest["provider"] = json!("cursor"),
                    "manifest-launch" => manifest["launch_id"] = json!("other-launch"),
                    _ => unreachable!(),
                }
                std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            }
        }
        let state_before = std::fs::read(&state_path).ok();
        let hooks_before = std::fs::read(fixture.hook_path()).unwrap();
        assert!(fixture.load().is_err(), "{field}");
        assert_eq!(std::fs::read(&state_path).ok(), state_before, "{field}");
        assert_eq!(
            std::fs::read(fixture.hook_path()).unwrap(),
            hooks_before,
            "{field}"
        );
    }
}

#[cfg(unix)]
#[test]
fn bus_resume_refuses_callback_spool_symlink_escape_before_touching_capture() {
    let fixture = Fixture::new(Provider::Codex);
    let spool = fixture.root.join("callbacks/owned-launch");
    let outside = fixture.root.join("other-capture");
    std::fs::rename(&spool, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &spool).unwrap();
    let state_before = std::fs::read(fixture.root.join("state.json")).unwrap();
    let hooks_before = std::fs::read(fixture.hook_path()).unwrap();
    assert_eq!(
        fixture.load().unwrap_err(),
        "Bus resume callback directory escaped its saved root"
    );
    assert_eq!(
        std::fs::read(fixture.root.join("state.json")).unwrap(),
        state_before
    );
    assert_eq!(std::fs::read(fixture.hook_path()).unwrap(), hooks_before);
}

#[test]
fn permission_profile_resume_restores_service_identity_and_current_executable_rules() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture
        .state
        .mark_permission_handler(fixture.agent)
        .unwrap();
    fixture.save();
    let extras = fixture.load().unwrap();
    assert!(extras.env.contains(&(
        "BUS_PERMISSION_HANDLER_ID".into(),
        fixture.agent.0.to_string()
    )));
    assert!(extras.env.iter().any(|(k, v)| k == "PATH"
        && std::env::split_paths(std::ffi::OsStr::new(v))
            .next()
            .as_deref()
            == fixture.binary.parent()));
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.hook_path()).unwrap()).unwrap();
    assert!(settings["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap().contains("agent permission-decision")));
}

#[test]
fn permission_profile_resume_refuses_observed_cwd_drift_before_capture_rewrite() {
    let fixture = Fixture::new(Provider::Codex);
    let bytes = std::fs::read(fixture.hook_path()).unwrap();
    let mut context = fixture.context();
    context.cwd = fixture.root.clone();
    std::fs::create_dir_all(fixture.root.join(".codex")).unwrap();
    std::fs::write(fixture.root.join(".codex/hooks.json"), &bytes).unwrap();
    assert!(
        load(&context, &fixture.facts, &fixture.plan).is_err(),
        "resume silently widened root"
    );
    assert_eq!(std::fs::read(fixture.hook_path()).unwrap(), bytes);
}

#[test]
fn permission_profile_resume_new_binary_overlap_downgrades_codex() {
    let mut fixture = Fixture::new(Provider::Codex);
    let scratch = fixture.root.join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    value["agents"][fixture.agent.0.to_string()]["scratch_dir"] =
        json!(scratch.canonicalize().unwrap());
    value["agents"][fixture.agent.0.to_string()]["launch_profile"] =
        json!("codex-workspace-write-v1");
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    let mut context = fixture.context();
    context.binary = fixture.project.join("target/debug/bus");
    std::fs::create_dir_all(context.binary.parent().unwrap()).unwrap();
    std::fs::write(&context.binary, b"fixture binary").unwrap();
    let extras = load(&context, &fixture.facts, &fixture.plan).unwrap();
    assert!(
        extras
            .args
            .windows(2)
            .any(|args| args == ["--sandbox", "read-only"]),
        "new running binary must remain protected: {:?}",
        extras.args
    );
}

#[test]
fn permission_profile_resume_rebinds_claude_worker_protected_paths() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let scratch = fixture.root.join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    value["agents"][fixture.agent.0.to_string()]["scratch_dir"] =
        json!(scratch.canonicalize().unwrap());
    value["agents"][fixture.agent.0.to_string()]["launch_profile"] =
        json!("claude-additive-sandbox-regular-approval-v1");
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    let mut context = fixture.context();
    context.binary = fixture.project.join("new-target/debug/bus");
    load(&context, &fixture.facts, &fixture.plan).unwrap();
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.hook_path()).unwrap()).unwrap();
    assert!(settings["sandbox"]["filesystem"]["denyWrite"]
        .as_array()
        .unwrap()
        .contains(&json!(context.binary)));
}

#[test]
fn permission_profile_service_socket_exception_is_exact_and_preserves_inherited_sandbox() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture
        .state
        .mark_permission_handler(fixture.agent)
        .unwrap();
    fixture.save();
    let inherited = json!({
        "enabled": true,
        "filesystem": {"denyWrite": ["/inherited/protected"], "allowWrite": ["/inherited/work"]},
        "network": {"allowAllUnixSockets": false, "allowedDomains": ["restricted.example"]}
    });
    let mut settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.hook_path()).unwrap()).unwrap();
    settings["sandbox"] = inherited.clone();
    std::fs::write(fixture.hook_path(), serde_json::to_vec(&settings).unwrap()).unwrap();
    fixture.load().unwrap();
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.hook_path()).unwrap()).unwrap();
    assert_eq!(
        settings["sandbox"]["network"]["allowUnixSockets"],
        json!([fixture.root.canonicalize().unwrap().join("control.sock")])
    );
    assert_eq!(settings["sandbox"]["enabled"], inherited["enabled"]);
    assert_eq!(settings["sandbox"]["filesystem"], inherited["filesystem"]);
    assert_eq!(
        settings["sandbox"]["network"]["allowAllUnixSockets"],
        inherited["network"]["allowAllUnixSockets"]
    );
    assert_eq!(
        settings["sandbox"]["network"]["allowedDomains"],
        inherited["network"]["allowedDomains"]
    );
}

#[test]
fn permission_profile_service_state_without_args_has_exact_allow_rule() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture
        .state
        .mark_permission_handler(fixture.agent)
        .unwrap();
    fixture.save();
    fixture.load().unwrap();
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.hook_path()).unwrap()).unwrap();
    let prefix = crate::agents::providers::launch::profile::cli_prefix(&fixture.binary);
    assert!(settings["permissions"]["allow"]
        .as_array()
        .unwrap()
        .contains(&json!(format!("Bash({prefix} state)"))));
}

#[test]
fn permission_profile_codex_resume_includes_audited_shared_root() {
    let mut fixture = Fixture::new(Provider::Codex);
    let shared = fixture.root.with_extension("shared-work");
    std::fs::create_dir(&shared).unwrap();
    let canonical = shared.canonicalize().unwrap();
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    let agent = &mut value["agents"][fixture.agent.0.to_string()];
    agent["launch_profile"] = json!("codex-workspace-write-v1");
    agent["allowed_roots"] = json!([{"path":canonical,"granted_by":99,"granted_at_ms":1}]);
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    let extras = fixture.load().unwrap();
    assert!(extras.args.windows(2).any(|a| a[0] == "--add-dir" && a[1] == canonical.to_string_lossy()));
    std::fs::remove_dir_all(shared).unwrap();
}

#[test]
fn permission_profile_review_codex_orchestrator_resume_keeps_native_approval() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new(Provider::Codex);
        let room = fixture.state.agent(fixture.agent).unwrap().room_id;
        let master = fixture.state.ensure_master_room();
        let mut value = serde_json::to_value(&fixture.state).unwrap();
        let agent = &mut value["agents"][fixture.agent.0.to_string()];
        agent["room_id"] = json!(master);
        agent["launch_profile"] = if legacy {
            json!(null)
        } else {
            json!("orchestrator-native-approval-v1")
        };
        if legacy {
            agent["launch_cwd"] = json!(null);
        }
        fixture.state = serde_json::from_value(value).unwrap();
        fixture
            .state
            .bind_orchestrator(fixture.agent, room)
            .unwrap();
        fixture.facts.agent_name = Some(identity::managed_name(master, fixture.agent));
        fixture.save();
        let extras = fixture.load().unwrap();
        assert!(
            !extras.args.iter().any(|a| a == "--sandbox"
                || a == "--ask-for-approval"
                || a.starts_with("sandbox_workspace_write.")),
            "orchestrator worker settings, legacy={legacy}: {:?}",
            extras.args
        );
    }
}

#[test]
fn permission_profile_review_resume_inside_live_data_is_read_only() {
    let mut fixture = Fixture::new(Provider::Codex);
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    value["agents"][fixture.agent.0.to_string()]["launch_profile"] =
        json!("codex-workspace-write-v1");
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    let extras = fixture.load().unwrap();
    assert!(
        extras
            .args
            .windows(2)
            .any(|a| a == ["--sandbox", "read-only"]),
        "worker inside session data received {:?}",
        extras.args
    );
}

#[test]
fn scoped_codex_worker_launched_in_git_subdirectory_resumes_from_its_repository_root() {
    let mut fixture = Fixture::new(Provider::Codex);
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&fixture.project)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(status.success());
    let subdirectory = fixture.project.join("crates/worker");
    std::fs::create_dir_all(&subdirectory).unwrap();
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    let agent = &mut value["agents"][fixture.agent.0.to_string()];
    agent["cwd"] = json!(subdirectory);
    agent["launch_cwd"] = json!(subdirectory.canonicalize().unwrap());
    agent["launch_profile"] = json!("codex-workspace-write-v1");
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    let mut context = fixture.context();
    // The native server derives the resume project from the pane cwd's git toplevel.
    context.project = crate::agents::providers::launch::project_root(&subdirectory);
    context.cwd = subdirectory.clone();
    assert_ne!(context.project, subdirectory.canonicalize().unwrap());
    let result = load(&context, &fixture.facts, &fixture.plan);
    assert!(
        result.is_ok(),
        "worker launched in a repository subdirectory could not resume: {result:?}"
    );
    let extras = result.unwrap();
    // This fixture repository is inside its isolated Bus data root, so resume
    // must preserve the protected-path downgrade even when cwd is a subdirectory.
    assert!(extras
        .args
        .windows(2)
        .any(|args| args == ["--sandbox", "read-only"]));
    assert!(extras
        .args
        .windows(2)
        .any(|args| args == ["--ask-for-approval", "on-request"]));
}

#[test]
fn scoped_codex_worker_resumes_after_its_private_scratch_was_removed_by_temp_cleanup() {
    let mut fixture = Fixture::new(Provider::Codex);
    let scratch = fixture.root.join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let scratch = scratch.canonicalize().unwrap();
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    value["agents"][fixture.agent.0.to_string()]["scratch_dir"] = json!(scratch);
    value["agents"][fixture.agent.0.to_string()]["launch_profile"] =
        json!("codex-workspace-write-v1");
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    // Reboot or an OS temp cleaner removes the idle private scratch directory.
    std::fs::remove_dir(&scratch).unwrap();
    let extras = fixture.load();
    assert!(
        extras.is_ok(),
        "removed scratch blocked the worker's conversation resume: {extras:?}"
    );
    let tmpdir = extras
        .unwrap()
        .env
        .into_iter()
        .find(|(key, _)| key == "TMPDIR")
        .map(|(_, value)| PathBuf::from(value))
        .expect("scoped worker keeps a private TMPDIR");
    assert_eq!(tmpdir, scratch);
    assert!(tmpdir.is_dir(), "TMPDIR {tmpdir:?} does not exist");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(tmpdir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[cfg(unix)]
#[test]
fn permission_profile_resume_refuses_replaced_scratch_symlink_before_capture_rewrite() {
    let mut fixture = Fixture::new(Provider::Codex);
    let scratch = fixture.root.join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let scratch = scratch.canonicalize().unwrap();
    let mut value = serde_json::to_value(&fixture.state).unwrap();
    value["agents"][fixture.agent.0.to_string()]["scratch_dir"] = json!(scratch);
    fixture.state = serde_json::from_value(value).unwrap();
    fixture.save();
    let bytes = std::fs::read(fixture.hook_path()).unwrap();
    std::fs::remove_dir(&scratch).unwrap();
    std::os::unix::fs::symlink(&fixture.project, &scratch).unwrap();
    assert_eq!(
        fixture.load().unwrap_err(),
        "Bus resume scratch escaped its frozen path"
    );
    assert_eq!(std::fs::read(fixture.hook_path()).unwrap(), bytes);
}
