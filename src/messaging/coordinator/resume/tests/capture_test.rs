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
