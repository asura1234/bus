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
fn claude_adapter_accepts_configuration_and_permission_overrides() {
    assert!(validate_args(&argv(&[
        "--model=sonnet",
        "--effort",
        "xhigh",
        "--verbose",
        "--ax-screen-reader",
        "--append-system-prompt",
        "Use literal @mentions and paths",
        "--system-prompt=custom",
        "--dangerously-skip-permissions",
        "--permission-mode=bypassPermissions",
        "--settings=/tmp/hooks.json",
    ]))
    .is_ok());
    for arg in [
        "--bare",
        "--safe-mode",
        "--bg",
        "--background",
        "-p",
        "--print",
    ] {
        assert!(validate_args(&argv(&[arg])).is_err(), "{arg}");
    }
}

#[test]
fn claude_adapter_leaves_provider_validation_to_claude_but_rejects_nul() {
    for args in [
        argv(&["--model"]),
        argv(&["--model="]),
        argv(&["--model", "--verbose"]),
        argv(&["--verbose=true"]),
        argv(&["--effort=unknown"]),
    ] {
        assert!(validate_args(&args).is_ok(), "{args:?}");
    }
    assert!(validate_args(&argv(&["--model=sonnet\0--settings"])).is_err());
    assert!(validate_args(&argv(&["--model", "sonnet\0"])).is_err());
}

#[test]
fn claude_adapter_resume_argv_is_separate_from_user_options_and_has_no_runtime_flags() {
    let session = "160d1f8b-9023-44b8-9bc7-24333effb185";
    assert_eq!(resume_args(session), argv(&["--resume", session]));
    assert!(runtime_args().is_empty());
}

#[test]
fn claude_adapter_adoption_extracts_one_uuid_and_leaves_native_selectors_untouched() {
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
        let original = args.clone();
        assert_eq!(take_adopted_session(&mut args).unwrap(), None);
        assert_eq!(args, original);
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
    let mut expected_env = spec.hooks.env();
    expected_env.extend(launch::runtime_env(ProviderKind::ClaudeCode));
    assert_eq!(result.env, expected_env);
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
fn claude_capture_settings_precede_a_literal_argument_tail() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec();
    spec.extra_args = "-- '--dangerously-skip-permissions literal prompt'".into();
    let initial = argv(&["--", "--dangerously-skip-permissions literal prompt"]);
    let result = prepare(&spec, fixture.0.clone(), initial, None).unwrap();
    let settings = spec.hooks.spool.join("claude-settings.json");
    assert_eq!(
        result.args,
        argv(&[
            "--settings",
            settings.to_str().unwrap(),
            "--",
            "--dangerously-skip-permissions literal prompt",
        ])
    );
}

#[test]
fn codexup_all_provider_claude_disables_updates_without_overriding_explicit_env() {
    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    let old = std::env::var_os("DISABLE_AUTOUPDATER");
    for setting in [None, Some("0"), Some("1")] {
        match setting {
            Some(value) => std::env::set_var("DISABLE_AUTOUPDATER", value),
            None => std::env::remove_var("DISABLE_AUTOUPDATER"),
        }
        let fixture = Fixture::new();
        let result = prepare(&fixture.spec(), fixture.0.clone(), Vec::new(), None).unwrap();
        let actual = result.env.get("DISABLE_AUTOUPDATER").map(String::as_str);
        let expected = setting.unwrap_or("1");
        // Restore first so a red assertion cannot pollute other tests.
        match &old {
            Some(value) => std::env::set_var("DISABLE_AUTOUPDATER", value),
            None => std::env::remove_var("DISABLE_AUTOUPDATER"),
        }
        assert_eq!(actual, Some(expected));
    }
}
