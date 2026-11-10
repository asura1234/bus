use super::{hook_json::HookContract, launch, parse, spool::Parsed, ProviderKind};
use crate::agents::providers::hook_json::HookContext;
use crate::agents::resume::catalog::{self, AgentSessionRef};
use crate::agents::{identify_agent, parse_agent_label, AgentKind};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

struct Stub {
    root: PathBuf,
    _path: crate::utils::env::PathOverride,
}

impl Stub {
    fn new() -> Self {
        let root = crate::utils::test_temp::unique_temp_path("bus-claude-ds");
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("claude-ds");
        std::fs::write(&binary, "#!/bin/sh\nexit 91\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _path: crate::utils::env::PathOverride::set(&root),
            root,
        }
    }

    fn spec(&self, extra_args: &str) -> launch::LaunchSpec {
        let cwd = self.root.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        launch::LaunchSpec {
            provider: ProviderKind::ClaudeDs,
            cwd: cwd.to_string_lossy().into_owned(),
            extra_args: extra_args.into(),
            consent_project_hooks: false,
            hooks: HookContext {
                binary: PathBuf::from("/tmp/bus-claude-ds-fixture"),
                spool: self.root.join("spool"),
                launch_id: "claude-ds-launch".into(),
            },
        }
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn claude_ds_label_and_executable_stay_distinct_from_claude() {
    assert_eq!(ProviderKind::ClaudeDs.label(), "claude-ds");
    assert_eq!(ProviderKind::ClaudeDs.executable(), "claude-ds");
    assert_eq!(launch::executable(ProviderKind::ClaudeDs), "claude-ds");
    assert_eq!(
        launch::selected_executable(ProviderKind::ClaudeDs, &Default::default()).unwrap(),
        Some("claude-ds".into())
    );
    assert_ne!(
        ProviderKind::ClaudeDs.label(),
        ProviderKind::ClaudeCode.label()
    );
    assert!(ProviderKind::ClaudeDs.uses_claude_harness());
}

#[test]
fn claude_ds_detection_uses_the_claude_harness() {
    assert_eq!(identify_agent("claude-ds"), Some(AgentKind::Claude));
    assert_eq!(parse_agent_label("claude-ds"), Some(AgentKind::Claude));
    assert_eq!(
        identify_agent("/Users/dylanliu/.local/bin/claude-ds"),
        Some(AgentKind::Claude)
    );
    assert!(crate::agents::same_detected_agent("claude", "claude-ds"));
}

#[test]
fn claude_ds_parses_claude_hooks_and_keeps_the_claude_hook_command() {
    let session = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "session",
        "source": "startup"
    });
    assert_eq!(
        parse(ProviderKind::ClaudeDs, &session).unwrap(),
        Parsed::Session {
            session: "session".into(),
            source: Some("startup".into())
        }
    );
    let contract = HookContract::for_provider(ProviderKind::ClaudeDs);
    assert_eq!(
        contract,
        HookContract::for_provider(ProviderKind::ClaudeCode)
    );
    assert_eq!(contract.adapter, "claude-hook");
}

#[test]
fn claude_ds_fresh_launch_installs_claude_hooks_without_consent_or_wrapper_flags() {
    let stub = Stub::new();
    std::fs::create_dir_all(stub.root.join("spool")).unwrap();
    let spec = stub.spec("");
    assert!(launch::setup_notice(
        spec.provider,
        std::path::Path::new(&spec.cwd),
        &spec.hooks.binary
    )
    .is_none());
    let prepared = launch::prepare(&spec, |_| Ok(())).unwrap();
    assert!(prepared.args.iter().any(|arg| arg == "--settings"));
    for flag in ["--model", "--effort", "--autocompact"] {
        assert!(
            !prepared
                .args
                .iter()
                .any(|arg| arg == flag || arg.starts_with(&format!("{flag}="))),
            "Bus must leave {flag} to the claude-ds wrapper: {:?}",
            prepared.args
        );
    }
    assert!(launch::runtime_args(ProviderKind::ClaudeDs).is_empty());
    assert_eq!(
        prepared.env.get("DISABLE_AUTOUPDATER").map(String::as_str),
        Some("1")
    );
    let settings = spec.hooks.spool.join("claude-settings.json");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    let command = document["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(command.contains("--bus-callback claude-hook"), "{command}");
}

#[test]
fn claude_ds_resume_uses_resume_and_the_wrapper_command() {
    let session = "160d1f8b-9023-44b8-9bc7-24333effb185";
    let stub = Stub::new();
    std::fs::create_dir_all(stub.root.join("spool")).unwrap();
    let spec = stub.spec(&format!("--resume {session}"));
    let prepared = launch::prepare(&spec, |_| Ok(())).unwrap();
    assert_eq!(prepared.adopted_session.as_deref(), Some(session));
    assert!(prepared
        .args
        .windows(2)
        .any(|pair| { pair[0] == "--resume" && pair[1] == session }));
    assert!(prepared.args.iter().any(|arg| arg == "--settings"));
    for flag in ["--model", "--effort", "--autocompact"] {
        assert!(!prepared.args.iter().any(|arg| arg == flag), "{flag}");
    }
    let plan = catalog::plan(
        "herdr:claude-ds",
        "claude-ds",
        &AgentSessionRef::id(session.to_owned()).unwrap(),
    )
    .unwrap();
    assert_eq!(plan.argv, argv(&["claude-ds", "--resume", session]));
}
