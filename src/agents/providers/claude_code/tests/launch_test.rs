use super::{prepare, resume_args, runtime_args, take_adopted_session};
use crate::agents::providers::{
    hook_json::HookContext,
    launch::{self, LaunchSpec},
    ProviderKind,
};
use serde_json::{json, Value};
use std::path::PathBuf;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn validate_args(args: &[String]) -> Result<(), String> {
    launch::validate_args(ProviderKind::ClaudeCode, args)
}

#[test]
fn claude_adapter_accepts_interactive_values_and_rejects_owned_settings_and_trust_overrides() {
    assert!(validate_args(&argv(&[
        "--model=sonnet",
        "--effort",
        "xhigh",
        "--verbose",
        "--ax-screen-reader",
        "--append-system-prompt",
        "Use literal @mentions and paths",
        "--system-prompt=custom",
    ]))
    .is_ok());
    for arg in [
        "--bare",
        "--safe-mode",
        "--bg",
        "--background",
        "--dangerously-skip-permissions",
        "--allowed-tools",
        "--settings=/tmp/hooks.json",
        "-phello",
        "--print=hello",
        "review",
        "--unknown-option=value",
        "-m",
        "--continue",
        "--fork-session",
        "--session-id",
        "--resume",
    ] {
        assert!(validate_args(&argv(&[arg])).is_err(), "{arg}");
    }
}

#[test]
fn claude_adapter_retains_missing_value_choice_and_nul_refusals() {
    for args in [
        argv(&["--model"]),
        argv(&["--model="]),
        argv(&["--model", "--verbose"]),
        argv(&["--verbose=true"]),
        argv(&["--effort=unknown"]),
        argv(&["--model=sonnet\0--settings"]),
        argv(&["--model", "sonnet\0"]),
    ] {
        assert!(validate_args(&args).is_err(), "{args:?}");
    }
    assert_eq!(
        validate_args(&argv(&["--effort=unknown"])).unwrap_err(),
        "Launch option --effort must be one of: low, medium, high, xhigh, max"
    );
    assert_eq!(
        validate_args(&argv(&["--model", "--verbose"])).unwrap_err(),
        "Launch option --model requires a nonempty value, not another option"
    );
}

#[test]
fn claude_adapter_resume_argv_is_separate_from_user_options_and_has_no_runtime_flags() {
    let session = "160d1f8b-9023-44b8-9bc7-24333effb185";
    assert_eq!(resume_args(session), argv(&["--resume", session]));
    assert!(runtime_args().is_empty());
}

#[test]
fn claude_adapter_adoption_extracts_one_uuid_without_accepting_a_picker_or_duplicate() {
    let session = "160d1f8b-9023-44b8-9bc7-24333effb185";
    for mut args in [
        argv(&["--model", "sonnet", "--resume", session]),
        argv(&[&format!("--resume={session}"), "--model", "sonnet"]),
    ] {
        assert_eq!(
            take_adopted_session(&mut args).unwrap(),
            Some(session.into())
        );
        assert_eq!(args, argv(&["--model", "sonnet"]));
    }
    for mut args in [
        argv(&["--resume"]),
        argv(&["--resume=not-a-uuid"]),
        argv(&["--resume", "--last"]),
        argv(&["--resume", session, "--resume", session]),
    ] {
        assert!(take_adopted_session(&mut args).is_err());
    }
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("bus-claude-adapter-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(path.join("spool")).unwrap();
        std::fs::create_dir_all(path.join(".claude")).unwrap();
        Self(path)
    }

    fn spec(&self) -> LaunchSpec {
        LaunchSpec {
            permission_handler: false,
            orchestrator: false,
            provider: ProviderKind::ClaudeCode,
            cwd: self.0.to_string_lossy().into_owned(),
            extra_args: "--model sonnet".into(),
            consent_project_hooks: false,
            hooks: HookContext {
                binary: PathBuf::from("/tmp/a bus fixture"),
                spool: self.0.join("spool"),
                launch_id: "claude-launch".into(),
            },
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn claude_adapter_settings_are_per_launch_preserve_user_hooks_and_need_no_project_consent() {
    let fixture = Fixture::new();
    let spec = fixture.spec();
    let local = fixture.0.join(".claude/settings.local.json");
    let local_bytes = br#"{"statusLine":{"type":"command","command":"my-local-hud","padding":3},"permissions":{"allow":["Read"]}}"#;
    std::fs::write(&local, local_bytes).unwrap();
    let settings = spec.hooks.spool.join("claude-settings.json");
    let user_hook = json!({"hooks":[{"type":"command","command":"user-stop"}]});
    std::fs::write(
        &settings,
        serde_json::to_vec(&json!({"custom":42,"hooks":{"Stop":[user_hook]}})).unwrap(),
    )
    .unwrap();
    let session = "160d1f8b-9023-44b8-9bc7-24333effb185";
    let initial = argv(&["--resume", session, "--model", "sonnet"]);
    let result = prepare(
        &spec,
        fixture.0.clone(),
        initial.clone(),
        Some(session.into()),
    )
    .unwrap();
    assert_eq!(result.cwd, fixture.0);
    assert_eq!(result.adopted_session.as_deref(), Some(session));
    let mut expected = initial;
    expected.extend(["--settings".into(), settings.to_string_lossy().into_owned()]);
    assert_eq!(result.args, expected);
    assert_eq!(result.env, spec.hooks.env());
    let document: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert_eq!(document["custom"], 42);
    assert_eq!(document["hooks"]["Stop"][0], user_hook);
    assert_eq!(document["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(
        document["hooks"]["StopFailure"].as_array().unwrap().len(),
        1
    );
    assert_eq!(document["statusLine"]["padding"], 3);
    let original: Value = serde_json::from_slice(
        &std::fs::read(spec.hooks.spool.join("claude-statusline.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(original["command"], "my-local-hud");
    assert_eq!(std::fs::read(&local).unwrap(), local_bytes);
    assert!(!fixture.0.join(".claude/hooks.json").exists());
}

#[test]
fn claude_adapter_refuses_corrupt_launch_settings_before_installing_the_statusline() {
    let fixture = Fixture::new();
    let spec = fixture.spec();
    let path = spec.hooks.spool.join("claude-settings.json");
    std::fs::write(&path, b"corrupt original").unwrap();
    assert!(prepare(&spec, fixture.0.clone(), Vec::new(), None).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"corrupt original");
    assert!(!spec.hooks.spool.join("claude-statusline.json").exists());
}

#[test]
fn permission_profile_codex_defaults_scoped_scratch_and_rejects_outside_extra_dir() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec();
    spec.provider = ProviderKind::Codex;
    spec.consent_project_hooks = true;
    spec.extra_args.clear();
    let prepared = launch::prepare(&spec, |_| Ok(())).unwrap();
    assert!(
        prepared
            .args
            .windows(2)
            .any(|a| a == ["--sandbox", "workspace-write"]),
        "{:?}",
        prepared.args
    );
    assert!(prepared
        .args
        .windows(2)
        .any(|a| a == ["--ask-for-approval", "on-request"]));
    let scratch = PathBuf::from(prepared.env.get("TMPDIR").expect("private scratch"));
    assert!(scratch.is_dir());
    assert!(prepared
        .args
        .iter()
        .any(|a| a == "sandbox_workspace_write.exclude_slash_tmp=true"));
    assert!(prepared
        .args
        .iter()
        .any(|a| a == "sandbox_workspace_write.exclude_tmpdir_env_var=true"));
    assert!(prepared
        .args
        .iter()
        .any(|a| a == "sandbox_workspace_write.network_access=false"));
    assert!(prepared
        .args
        .iter()
        .any(|a| a.contains("sandbox_workspace_write.writable_roots=")
            && a.contains(&*scratch.to_string_lossy())));
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn permission_handler_service_read_tools_are_preallowed() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec();
    spec.permission_handler = true;
    spec.hooks.spool = fixture.0.join("callbacks/service");
    let prepared = launch::prepare(&spec, |_| Ok(())).unwrap();
    let settings: Value = serde_json::from_slice(
        &std::fs::read(spec.hooks.spool.join("claude-settings.json")).unwrap(),
    )
    .unwrap();
    let allow = settings["permissions"]["allow"].as_array().unwrap();
    for rule in [
        "Read",
        "Glob",
        "Grep",
        "Bash(cat *)",
        "Bash(ls *)",
        "Bash(realpath *)",
        "Bash(grep *)",
        "Bash(head *)",
        "Bash(test *)",
    ] {
        assert!(allow.contains(&json!(rule)), "missing {rule}: {allow:?}");
    }
    assert_eq!(settings["permissions"]["defaultMode"], "dontAsk");
    assert!(prepared
        .args
        .windows(2)
        .any(|a| a == ["--permission-mode", "dontAsk"]));
    assert!(prepared
        .args
        .windows(2)
        .any(|a| a == ["--tools", "Read,Glob,Grep,Bash"]));
    assert!(!prepared
        .args
        .iter()
        .any(|a| a == "--dangerously-skip-permissions"));
}

#[test]
fn permission_profile_outside_extra_dir_is_refused_before_launch_side_effects() {
    let fixture = Fixture::new();
    let sibling = Fixture::new();
    let mut spec = fixture.spec();
    spec.provider = ProviderKind::Codex;
    spec.consent_project_hooks = true;
    spec.extra_args = format!("--add-dir '{}'", sibling.0.display());
    let result = launch::prepare(&spec, |_| panic!("outside root reached initialization"));
    assert!(result.is_err(), "arbitrary sibling accepted: {result:?}");
    assert!(!fixture.0.join(".codex/hooks.json").exists());
}

#[test]
fn permission_profile_claude_workers_keep_regular_checks_and_protect_control_paths() {
    let fixture = Fixture::new();
    let spec = fixture.spec();
    let prepared = launch::prepare(&spec, |_| Ok(())).unwrap();
    let settings: Value = serde_json::from_slice(
        &std::fs::read(spec.hooks.spool.join("claude-settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["permissions"]["defaultMode"], "default");
    assert_eq!(settings["sandbox"]["enabled"], true);
    assert_eq!(settings["sandbox"]["autoAllowBashIfSandboxed"], false);
    assert!(settings["sandbox"]["filesystem"]["denyWrite"]
        .as_array()
        .unwrap()
        .contains(&json!(spec.hooks.binary)));
    assert!(settings["permissions"]["allow"]
        .as_array()
        .is_none_or(|a| a.is_empty()));
    std::fs::remove_dir_all(prepared.env.get("TMPDIR").unwrap()).unwrap();
}

#[test]
fn permission_profile_codex_protected_overlap_is_read_only_and_child_dirs_are_scoped() {
    let fixture = Fixture::new();
    let child = fixture.0.join("owned-child");
    std::fs::create_dir(&child).unwrap();
    let mut spec = fixture.spec();
    spec.provider = ProviderKind::Codex;
    spec.consent_project_hooks = true;
    spec.hooks.binary = child.join("bus");
    std::fs::write(&spec.hooks.binary, b"fixture binary").unwrap();
    spec.extra_args = format!("--add-dir '{}'", child.display());
    let prepared = launch::prepare(&spec, |_| Ok(())).unwrap();
    assert!(prepared
        .args
        .windows(2)
        .any(|a| a == ["--sandbox", "read-only"]));
    assert_eq!(prepared.profile, "codex-read-only-protected-overlap-v1");
    std::fs::remove_dir_all(prepared.scratch_dir.unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
fn permission_profile_symlink_extra_dir_cannot_escape_frozen_root() {
    let fixture = Fixture::new();
    let sibling = Fixture::new();
    let escaped = fixture.0.join("escaped");
    std::os::unix::fs::symlink(&sibling.0, &escaped).unwrap();
    let mut spec = fixture.spec();
    spec.provider = ProviderKind::Codex;
    spec.consent_project_hooks = true;
    spec.extra_args = format!("--add-dir={}", escaped.display());
    assert!(launch::prepare(&spec, |_| panic!("escape reached capture")).is_err());
    assert!(!fixture.0.join(".codex/hooks.json").exists());
}

#[cfg(unix)]
#[test]
fn permission_profile_extra_dir_argv_freezes_canonical_descendant() {
    let fixture = Fixture::new();
    let child = fixture.0.join("owned-child");
    std::fs::create_dir(&child).unwrap();
    let alias = fixture.0.join("alias");
    std::os::unix::fs::symlink(&child, &alias).unwrap();
    let mut spec = fixture.spec();
    spec.provider = ProviderKind::Codex;
    spec.consent_project_hooks = true;
    spec.extra_args = format!("--add-dir={}", alias.display());
    let prepared = launch::prepare(&spec, |_| Ok(())).unwrap();
    assert!(
        prepared.args.contains(&format!(
            "--add-dir={}",
            child.canonicalize().unwrap().display()
        )),
        "alias can change after preparation: {:?}",
        prepared.args
    );
    std::fs::remove_dir_all(prepared.scratch_dir.unwrap()).unwrap();
}

fn assert_worker_inside_protected_root_read_only(target: bool) {
    let fixture = Fixture::new();
    let mut spec = fixture.spec();
    spec.provider = ProviderKind::Codex;
    spec.consent_project_hooks = true;
    spec.extra_args.clear();
    let protected = fixture.0.join(if target { "target" } else { "session" });
    let cwd = protected.join("worker");
    std::fs::create_dir_all(&cwd).unwrap();
    if target {
        spec.hooks.binary = protected.join("debug/bus");
        std::fs::create_dir_all(spec.hooks.binary.parent().unwrap()).unwrap();
        std::fs::write(&spec.hooks.binary, b"fixture binary").unwrap();
    } else {
        spec.hooks.spool = protected.join("callbacks/launch");
        std::fs::create_dir_all(&spec.hooks.spool).unwrap();
    }
    spec.cwd = cwd.to_string_lossy().into_owned();
    let prepared = launch::prepare(&spec, |_| Ok(())).unwrap();
    assert!(
        prepared
            .args
            .windows(2)
            .any(|a| a == ["--sandbox", "read-only"]),
        "inside protected root received {:?}",
        prepared.args
    );
    std::fs::remove_dir_all(prepared.scratch_dir.unwrap()).unwrap();
}

#[test]
fn permission_profile_review_launch_inside_live_target_is_read_only() {
    assert_worker_inside_protected_root_read_only(true);
}

#[test]
fn permission_profile_review_launch_inside_live_data_is_read_only() {
    assert_worker_inside_protected_root_read_only(false);
}
