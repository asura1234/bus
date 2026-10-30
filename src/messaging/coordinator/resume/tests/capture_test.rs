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

#[test]
fn yolo_resumed_codex_master_uses_current_run_without_persisting() {
    use crate::agents::{
        providers::{hook_json, spool, ProviderKind},
        resume::catalog::{self, AgentSessionRef, PersistedAgentSession},
    };
    use crate::messaging::{
        coordinator::resume::{self, NativeResumeContext, NativeResumeFacts},
        identity,
        model::{AgentRuntimeIdentity, BusState, Provider},
        storage::state_store::JsonStore,
    };

    struct Cleanup {
        root: PathBuf,
        previous_yolo: Option<std::ffi::OsString>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            match &self.previous_yolo {
                Some(value) => std::env::set_var("BUS_YOLO", value),
                None => std::env::remove_var("BUS_YOLO"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let cleanup = Cleanup {
        root: crate::utils::test_temp::unique_temp_path("codex-master-resume"),
        previous_yolo: std::env::var_os("BUS_YOLO"),
    };
    let project = cleanup.root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let binary = std::env::current_exe().unwrap();
    let mut state = BusState::new();
    let room = state.create_room("saved-work").unwrap();
    let master = state.ensure_master_room();
    // A saved MASTER fixture bypasses only new-agent admission, which remains refused.
    assert!(crate::messaging::orchestration::check_new_orchestrator(Provider::Codex).is_err());
    let agent = state
        .create_agent(
            master,
            "saved-master",
            Provider::Codex,
            project.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, room).unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("saved-master-launch".into()),
                terminal_id: Some("saved-terminal".into()),
                pane_id: Some("saved-pane".into()),
                session_id: Some("saved-conversation".into()),
            },
        )
        .unwrap();
    JsonStore::new(cleanup.root.join("state.json"))
        .save(&state)
        .unwrap();
    let capture = cleanup.root.join("callbacks/saved-master-launch");
    spool::initialize(
        &capture,
        &spool::Manifest {
            routing_key: spool::RoutingKey(agent.0),
            provider: ProviderKind::Codex,
            launch_id: "saved-master-launch".into(),
        },
    )
    .unwrap();
    hook_json::install_hooks(
        &project.join(".codex/hooks.json"),
        hook_json::HookContract::for_provider(ProviderKind::Codex),
        &binary,
    )
    .unwrap();
    let prompt =
        crate::messaging::orchestration::write_prompt(&capture, "Coordinate saved-work.").unwrap();
    let session = PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: AgentSessionRef::id("saved-conversation").unwrap(),
    };
    let plan = catalog::plan(&session.source, &session.agent, &session.session_ref).unwrap();
    let facts = NativeResumeFacts {
        agent_name: Some(identity::managed_name(master, agent)),
        managed_agent: Some("codex".into()),
        session: Some(session),
    };
    let context = NativeResumeContext {
        root: Some(cleanup.root.clone()),
        session_name: Some("bus".into()),
        config_root: cleanup.root.join("herdr-config"),
        project,
        binary,
    };
    for explicit in [
        None,
        Some("--yolo"),
        Some("--dangerously-bypass-approvals-and-sandbox"),
    ] {
        let options: Vec<&str> = explicit.into_iter().collect();
        let saved = serde_json::to_vec(&options).unwrap();
        std::fs::write(capture.join("launch-options.json"), &saved).unwrap();
        for enabled in [true, false, true] {
            if enabled {
                std::env::set_var("BUS_YOLO", "1");
            } else {
                std::env::remove_var("BUS_YOLO");
            }
            let extras = resume::load(&context, &facts, &plan).unwrap();
            let bypass: Vec<_> = extras
                .args
                .iter()
                .filter(|arg| {
                    matches!(
                        arg.as_str(),
                        "--yolo" | "--dangerously-bypass-approvals-and-sandbox"
                    )
                })
                .map(String::as_str)
                .collect();
            let expected: Vec<_> = explicit
                .or(enabled.then_some("--yolo"))
                .into_iter()
                .collect();
            assert_eq!(bypass, expected, "enabled={enabled}, explicit={explicit:?}");
            assert!(extras.args.iter().any(|arg| arg == "--no-daemon"));
            assert!(extras.args.iter().any(|arg| {
                arg.starts_with("developer_instructions=")
                    && arg.contains(&prompt.display().to_string())
            }));
            assert_eq!(
                std::fs::read(capture.join("launch-options.json")).unwrap(),
                saved
            );
        }
    }
}

// Catch bypass injection missing from either preparation or resume, duplicate
// aliases, and generated flags accidentally becoming durable user options.
#[test]
fn yolo_applies_to_new_and_resumed_agents_without_persisting_or_duplicating() {
    use crate::agents::providers::{hook_json::HookContext, launch};
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("BUS_YOLO");
    let old_path = std::env::var_os("PATH");
    let mut failures = Vec::new();
    for (provider, flag, aliases) in [
        (
            Provider::ClaudeCode,
            "--dangerously-skip-permissions",
            vec!["--dangerously-skip-permissions"],
        ),
        (
            Provider::Codex,
            "--yolo",
            vec!["--yolo", "--dangerously-bypass-approvals-and-sandbox"],
        ),
        (Provider::Cursor, "--force", vec!["--force", "--yolo", "-f"]),
    ] {
        for input in std::iter::once("").chain(aliases.iter().copied()) {
            let fixture = Fixture::new(provider);
            let kind = identity::provider_kind(provider);
            let spool = fixture.root.join("callbacks/owned-launch");
            let bin = fixture.root.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join(launch::executable(kind)), "test executable").unwrap();
            std::env::set_var("PATH", &bin);
            std::env::set_var("BUS_YOLO", "1");
            let prepared = launch::prepare(
                &launch::LaunchSpec {
                    provider: kind,
                    cwd: fixture.project.display().to_string(),
                    extra_args: input.into(),
                    consent_project_hooks: true,
                    hooks: HookContext {
                        binary: fixture.binary.clone(),
                        spool: spool.clone(),
                        launch_id: "owned-launch".into(),
                    },
                },
                |_| Ok(()),
            )
            .unwrap();
            let resumed = fixture.load().unwrap();
            for (phase, argv) in [("new", prepared.args), ("resume", resumed.args)] {
                let actual: Vec<_> = argv
                    .iter()
                    .filter(|a| aliases.contains(&a.as_str()))
                    .map(String::as_str)
                    .collect();
                let expected = if input.is_empty() { flag } else { input };
                if actual != [expected] {
                    failures.push(format!(
                        "{provider:?} {phase} input={input:?}: {actual:?}, want {expected}"
                    ));
                }
            }
            let saved = std::fs::read(spool.join("launch-options.json")).unwrap();
            assert_eq!(
                launch::saved_options(kind, &spool).unwrap(),
                if input.is_empty() {
                    Vec::<String>::new()
                } else {
                    vec![input.to_owned()]
                }
            );
            std::env::remove_var("BUS_YOLO");
            let normal = fixture.load().unwrap();
            let count = normal
                .args
                .iter()
                .filter(|a| aliases.contains(&a.as_str()))
                .count();
            assert_eq!(
                count,
                usize::from(!input.is_empty()),
                "{provider:?} subsequent normal run"
            );
            assert_eq!(
                std::fs::read(spool.join("launch-options.json")).unwrap(),
                saved
            );
        }
    }
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    if let Some(old_path) = old_path {
        std::env::set_var("PATH", old_path);
    } else {
        std::env::remove_var("PATH");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

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
            crate::agents::providers::ProviderKind::Grok | crate::agents::providers::ProviderKind::Kimi => panic!("This fixture covers the original providers; use dedicated Grok provider tests"),
            Provider::ClaudeCode => self
                .root
                .join("callbacks/owned-launch/claude-settings.json"),
            Provider::Codex => self.project.join(".codex/hooks.json"),
            Provider::Cursor => self.project.join(".cursor/hooks.json"),
        }
    }

    fn write_hooks(&self) {
        let (adapter, events): (&str, &[&str]) = match self.provider {
            crate::agents::providers::ProviderKind::Grok | crate::agents::providers::ProviderKind::Kimi => panic!("This fixture covers the original providers; use dedicated Grok provider tests"),
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
        let mut env: Vec<_> = [
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
        .into();
        env.extend(crate::agents::providers::launch::runtime_env(self.provider));
        env
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn resumed_launch_preserves_original_options_without_adding_bypass_flags() {
    use crate::agents::providers::{hook_json::HookContext, launch};
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    for (provider, options) in [
        (Provider::Codex, "--model gpt-test --add-dir '/work/sibling dir' --no-alt-screen"),
        (Provider::ClaudeCode, "--model sonnet --effort high --verbose"),
        (Provider::Cursor, "--model test --mode=plan"),
        (Provider::Codex, "--yolo -c 'model_reasoning_effort=\"high\"' --sandbox danger-full-access --ask-for-approval never"),
        (Provider::ClaudeCode, "--dangerously-skip-permissions --permission-mode bypassPermissions --settings /tmp/user.json"),
        (Provider::Cursor, "--yolo --force --sandbox disabled --approve-mcps"),
    ] {
        for adopted in [false, true] {
            let fixture = Fixture::new(provider);
            let kind = identity::provider_kind(provider);
            let session = "160d1f8b-9023-44b8-9bc7-24333effb185";
            let input = if adopted {
                match provider {
                    Provider::Codex => format!("resume {session} {options}"),
                    _ => format!("--resume {session} {options}"),
                }
            } else {
                options.into()
            };
            let bin = fixture.root.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join(launch::executable(kind)), "test executable").unwrap();
            let previous = std::env::var_os("PATH");
            std::env::set_var("PATH", &bin);
            let prepared = launch::prepare(&launch::LaunchSpec {
                provider: kind,
                cwd: fixture.project.display().to_string(),
                extra_args: input,
                consent_project_hooks: true,
                hooks: HookContext {
                    binary: fixture.binary.clone(),
                    spool: fixture.root.join("callbacks/owned-launch"),
                    launch_id: "owned-launch".into(),
                },
            }, |_| Ok(()));
            if let Some(previous) = previous {
                std::env::set_var("PATH", previous);
            } else {
                std::env::remove_var("PATH");
            }
            let mut prepared = prepared.unwrap();
            launch::take_adopted_session(kind, &mut prepared.args).unwrap();
            let resumed = fixture.load().unwrap();
            assert_eq!(resumed.args, prepared.args, "lost launch flags for {provider:?}");
            assert_eq!(launch::saved_options(kind, &fixture.root.join("callbacks/owned-launch")).unwrap(), launch::parse_path_tokens(options).unwrap());
        }
    }
}

#[test]
fn saved_resume_options_reject_only_launch_breakers_malformed_and_unreadable_records() {
    let fixture = Fixture::new(Provider::Codex);
    let path = fixture
        .root
        .join("callbacks/owned-launch/launch-options.json");
    for bytes in [
        r#"["exec"]"#,
        r#"["--unknown=has\u0000nul"]"#,
        "broken json",
        r#"{"options":[]}"#,
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(fixture.load().is_err(), "accepted {bytes}");
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(fixture.load().is_err());
}

#[test]
fn unstarted_resume_replays_saved_options() {
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
    fixture.save();
    fixture.facts.session = None;
    fixture.plan =
        crate::messaging::coordinator::resume::unstarted_codex_plan(&fixture.facts, None).unwrap();
    let path = fixture
        .root
        .join("callbacks/owned-launch/launch-options.json");
    std::fs::write(&path, r#"["--model","gpt-test"]"#).unwrap();
    assert_eq!(
        fixture.load().unwrap().args,
        [
            "--model",
            "gpt-test",
            "--no-daemon",
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            r#"tui.keymap.global.open_external_editor=["ctrl-g"]"#,
            "-c",
            r#"tui.keymap.editor.delete_backward=["backspace"]"#
        ]
    );
}

#[test]
fn resume_does_not_duplicate_an_explicit_private_runtime_option() {
    let fixture = Fixture::new(Provider::Codex);
    std::fs::write(
        fixture
            .root
            .join("callbacks/owned-launch/launch-options.json"),
        r#"["--yolo","--no-daemon"]"#,
    )
    .unwrap();
    assert_eq!(
        fixture.load().unwrap().args,
        [
            "--yolo",
            "--no-daemon",
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            r#"tui.keymap.global.open_external_editor=["ctrl-g"]"#,
            "-c",
            r#"tui.keymap.editor.delete_backward=["backspace"]"#
        ]
    );
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
        vec!["codex", "--yolo", "resume", "previous"],
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
            crate::agents::providers::ProviderKind::Grok | crate::agents::providers::ProviderKind::Kimi => panic!("This fixture covers the original providers; use dedicated Grok provider tests"),
                Provider::ClaudeCode => vec![
                    "--settings".to_owned(),
                    fixture.hook_path().to_string_lossy().into_owned(),
                ],
                // The shared app-server daemon runs hooks with the env of
                // whichever pane started it, misrouting this launch's callbacks.
                Provider::Codex => crate::agents::providers::launch::runtime_args(Provider::Codex),
                Provider::Cursor =>
                    crate::agents::providers::launch::runtime_args(Provider::Cursor),
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
fn unstarted_resume_from_saved_launch_argv_does_not_repeat_original_options() {
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
    fixture.save();
    fixture.facts.session = None;
    // The pane's persisted launch argv is the executable plus prepared args.
    let launch_argv: Vec<String> = ["codex", "--model", "gpt-test", "--no-daemon"]
        .map(str::to_owned)
        .into();
    fixture.plan = crate::messaging::coordinator::resume::unstarted_codex_plan(
        &fixture.facts,
        Some(&launch_argv),
    )
    .unwrap();
    std::fs::write(
        fixture
            .root
            .join("callbacks/owned-launch/launch-options.json"),
        r#"["--model","gpt-test"]"#,
    )
    .unwrap();
    let extras = fixture.load().unwrap();
    let argv: Vec<String> = fixture
        .plan
        .argv
        .iter()
        .chain(extras.args.iter())
        .cloned()
        .collect();
    let mut expected = launch_argv;
    expected.extend([
        "-c".into(),
        "check_for_update_on_startup=false".into(),
        "-c".into(),
        r#"tui.keymap.global.open_external_editor=["ctrl-g"]"#.into(),
        "-c".into(),
        r#"tui.keymap.editor.delete_backward=["backspace"]"#.into(),
    ]);
    assert_eq!(argv, expected);
}

#[test]
fn resume_of_a_started_conversation_does_not_replay_its_initial_prompt_or_selector() {
    use crate::agents::providers::{hook_json::HookContext, launch};
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let mut replayed = Vec::new();
    for (provider, input, launch_only) in [
        (
            Provider::Codex,
            "--yolo 'summarize the repo'",
            vec!["summarize the repo"],
        ),
        (Provider::Codex, "resume --last", vec!["resume", "--last"]),
        (
            Provider::ClaudeCode,
            "--dangerously-skip-permissions 'summarize the repo'",
            vec!["summarize the repo"],
        ),
    ] {
        let fixture = Fixture::new(provider);
        let kind = identity::provider_kind(provider);
        let bin = fixture.root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(launch::executable(kind)), "test executable").unwrap();
        let previous = std::env::var_os("PATH");
        std::env::set_var("PATH", &bin);
        let prepared = launch::prepare(
            &launch::LaunchSpec {
                provider: kind,
                cwd: fixture.project.display().to_string(),
                extra_args: input.into(),
                consent_project_hooks: true,
                hooks: HookContext {
                    binary: fixture.binary.clone(),
                    spool: fixture.root.join("callbacks/owned-launch"),
                    launch_id: "owned-launch".into(),
                },
            },
            |_| Ok(()),
        );
        if let Some(previous) = previous {
            std::env::set_var("PATH", previous);
        } else {
            std::env::remove_var("PATH");
        }
        prepared.unwrap();
        // The saved conversation already received this prompt or was chosen by
        // this selector; the resume plan names the conversation itself.
        let extras = fixture.load().unwrap();
        let argv: Vec<String> = fixture
            .plan
            .argv
            .iter()
            .chain(extras.args.iter())
            .cloned()
            .collect();
        if launch_only
            .iter()
            .any(|token| extras.args.iter().any(|arg| arg == token))
        {
            replayed.push(format!("{provider:?}: {argv:?}"));
        }
    }
    assert!(
        replayed.is_empty(),
        "resume replays launch-only tokens: {replayed:#?}"
    );
}

#[test]
fn started_resume_keeps_options_after_prompts_and_leaves_unknown_arity_untouched() {
    use crate::agents::providers::launch;
    for (provider, original, expected) in [
        (
            Provider::Codex,
            "'already submitted' --yolo -c 'key=\"resume\"' --sandbox danger-full-access",
            "--yolo -c 'key=\"resume\"' --sandbox danger-full-access",
        ),
        (
            Provider::ClaudeCode,
            "'already submitted' --permission-mode bypassPermissions --system-prompt '--resume'",
            "--permission-mode bypassPermissions --system-prompt '--resume'",
        ),
        (
            Provider::Cursor,
            "'already submitted' --yolo --sandbox disabled",
            "--yolo --sandbox disabled",
        ),
        (
            Provider::Codex,
            "--yolo resume --last -c key=value",
            "--yolo -c key=value",
        ),
        (
            Provider::ClaudeCode,
            "--resume named-session --continue --fork-session --dangerously-skip-permissions",
            "--dangerously-skip-permissions",
        ),
        (
            Provider::Cursor,
            "--resume named-session --continue --yolo",
            "--yolo",
        ),
        (
            Provider::Codex,
            "--future-config '--resume' value --yolo",
            "--future-config '--resume' value --yolo",
        ),
    ] {
        let fixture = Fixture::new(provider);
        let kind = identity::provider_kind(provider);
        let original = launch::parse_path_tokens(original).unwrap();
        let path = fixture
            .root
            .join("callbacks/owned-launch/launch-options.json");
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let mut expected = launch::parse_path_tokens(expected).unwrap();
        expected.extend(match provider {
            Provider::Grok | Provider::Kimi => {
                panic!("This fixture covers the original providers; use dedicated provider tests")
            }
            Provider::Codex => crate::agents::providers::launch::runtime_args(Provider::Codex),
            Provider::ClaudeCode => vec![
                "--settings".into(),
                fixture.hook_path().to_string_lossy().into_owned(),
            ],
            Provider::Cursor => crate::agents::providers::launch::runtime_args(Provider::Cursor),
        });
        assert_eq!(
            fixture.load().unwrap().args,
            expected,
            "{provider:?}: {original:?}"
        );
        assert_eq!(
            launch::saved_options(kind, path.parent().unwrap()).unwrap(),
            original
        );
    }
}

#[test]
fn unstarted_resume_preserves_literal_initial_input_with_capture_options_before_it() {
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
    fixture.save();
    fixture.facts.session = None;
    fixture.plan =
        crate::messaging::coordinator::resume::unstarted_codex_plan(&fixture.facts, None).unwrap();
    std::fs::write(
        fixture
            .root
            .join("callbacks/owned-launch/launch-options.json"),
        r#"["--yolo","--","first prompt"]"#,
    )
    .unwrap();
    assert_eq!(
        fixture.load().unwrap().args,
        [
            "--yolo",
            "--no-daemon",
            "-c",
            "check_for_update_on_startup=false",
            "-c",
            r#"tui.keymap.global.open_external_editor=["ctrl-g"]"#,
            "-c",
            r#"tui.keymap.editor.delete_backward=["backspace"]"#,
            "--",
            "first prompt"
        ]
    );
}

// Codex 0.163 rejects its bypass flag together with an explicit approval
// policy ("the argument '--dangerously-bypass-approvals-and-sandbox' cannot be
// used with '--ask-for-approval'"), so a yolo run must not produce that argv.
#[test]
fn yolo_resume_does_not_combine_codex_bypass_with_an_explicit_approval_policy() {
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("BUS_YOLO");
    std::env::set_var("BUS_YOLO", "1");
    let mut failures = Vec::new();
    for saved in [
        r#"["--ask-for-approval","on-request"]"#,
        r#"["-a","never"]"#,
        r#"["--ask-for-approval=on-request"]"#,
        r#"["--approve-for-me"]"#,
        r#"["-anever"]"#,
    ] {
        let fixture = Fixture::new(Provider::Codex);
        std::fs::write(
            fixture
                .root
                .join("callbacks/owned-launch/launch-options.json"),
            saved,
        )
        .unwrap();
        let args = fixture.load().unwrap().args;
        let bypass = args
            .iter()
            .any(|a| a == "--yolo" || a == "--dangerously-bypass-approvals-and-sandbox");
        let approval = args.iter().any(|a| {
            a == "-a" || a == "--ask-for-approval" || a.starts_with("--ask-for-approval=")
        });
        if !bypass
            || approval
            || args
                .iter()
                .any(|a| a == "--approve-for-me" || a.starts_with("-a"))
        {
            failures.push(format!("{saved} -> {args:?}"));
        }
        assert_eq!(
            std::fs::read_to_string(
                fixture
                    .root
                    .join("callbacks/owned-launch/launch-options.json")
            )
            .unwrap(),
            saved
        );
        std::env::remove_var("BUS_YOLO");
        let normal = fixture.load().unwrap().args;
        let original: Vec<String> = serde_json::from_str(saved).unwrap();
        assert_eq!(&normal[..original.len()], original);
        std::env::set_var("BUS_YOLO", "1");
    }
    std::env::remove_var("BUS_YOLO");
    if let Some(old) = old {
        std::env::set_var("BUS_YOLO", old);
    }
    assert!(
        failures.is_empty(),
        "Codex would refuse to start:\n{}",
        failures.join("\n")
    );
}
