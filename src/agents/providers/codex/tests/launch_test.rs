use super::{launch, system_prompt};
use crate::agents::providers::launch::LaunchOption;
use std::path::Path;

fn args(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| (*token).to_owned()).collect()
}

#[test]
fn codex_adoption_extracts_only_a_leading_uuid_resume_form() {
    let id = "160d1f8b-9023-44b8-9bc7-24333effb185";
    let mut input = args(&["resume", id, "-m", "gpt-test"]);
    assert_eq!(
        launch::take_adopted_session(&mut input).unwrap(),
        Some(id.into())
    );
    assert_eq!(input, args(&["-m", "gpt-test"]));
    assert_eq!(launch::resume_args(id), args(&["resume", id]));
    let mut value = args(&["--model", "resume"]);
    assert_eq!(launch::take_adopted_session(&mut value).unwrap(), None);
    assert_eq!(value, args(&["--model", "resume"]));
    for tokens in [
        vec!["resume"],
        vec!["resume", "--last"],
        vec!["resume", "not-a-uuid"],
    ] {
        assert!(launch::take_adopted_session(&mut args(&tokens)).is_err());
    }
    assert_eq!(
        launch::take_adopted_session(&mut args(&["resume", id, "resume", id])).unwrap_err(),
        "Give at most one session to resume"
    );
}

#[test]
fn codex_interactive_metadata_keeps_daemon_and_permission_flags_bus_owned() {
    assert_eq!(launch::runtime_args(), args(&["--no-daemon"]));
    for flag in ["--no-alt-screen", "--strict-config", "--oss", "--search"] {
        assert!(matches!(
            launch::supported_option(flag),
            Some(LaunchOption::Flag)
        ));
    }
    for flag in ["--model", "-m"] {
        assert!(matches!(
            launch::supported_option(flag),
            Some(LaunchOption::Value)
        ));
    }
    assert!(matches!(
        launch::supported_option("--local-provider"),
        Some(LaunchOption::Choice(["ollama", "lmstudio"]))
    ));
    for flag in [
        "--no-daemon",
        "--sandbox",
        "--ask-for-approval",
        "-a",
        "-s",
        "-c",
        "review",
        "e",
    ] {
        assert!(launch::supported_option(flag).is_none(), "{flag}");
    }
}

#[test]
fn codex_setup_notice_preserves_project_consent_and_callback_adapter() {
    let notice = launch::setup_notice(Path::new("/project"));
    assert_eq!(notice.path, Path::new("/project/.codex/hooks.json"));
    assert!(notice
        .message
        .contains("Trust this project and all three definitions in Codex."));
    assert!(notice.message.contains("Existing hooks/notify remain."));
    assert!(notice.message.ends_with("--bus-callback codex-hook."));
}

#[test]
fn codex_prompt_reference_is_short_escaped_and_skipped_for_adopted_threads() {
    let path = Path::new("/tmp/a path/it\"s prompt.md");
    let argv = system_prompt::prompt_args(path, false).unwrap().unwrap();
    assert_eq!(argv[0], "-c");
    let encoded = argv[1].strip_prefix("developer_instructions=").unwrap();
    let decoded: String = serde_json::from_str(encoded).unwrap();
    assert_eq!(decoded, format!(
        "You are a Bus orchestrator. Your system prompt is the file {}. Read all of it before you act on any message, and follow it for the whole session.",
        path.display()
    ));
    assert!(system_prompt::prompt_args(path, true).unwrap().is_none());
    let long = format!("/tmp/{}/prompt.md", "segment/".repeat(200));
    let argv = system_prompt::prompt_args(Path::new(&long), false)
        .unwrap()
        .unwrap();
    assert!(argv[1].contains(&long));
    assert!(!argv[1].contains("Bus: this is your system prompt"));
}
