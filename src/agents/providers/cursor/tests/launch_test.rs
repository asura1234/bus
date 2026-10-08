use super::launch;
use crate::agents::providers::launch::LaunchOption;
use std::path::Path;

fn args(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| (*token).to_owned()).collect()
}

#[test]
fn cursor_adoption_accepts_separate_and_attached_resume_at_any_position() {
    let id = "160d1f8b-9023-44b8-9bc7-24333effb185";
    for mut input in [
        args(&["--model", "sonnet", "--resume", id]),
        args(&[&format!("--resume={id}"), "--model", "sonnet"]),
    ] {
        assert_eq!(
            launch::take_adopted_session(&mut input).unwrap(),
            Some(id.into())
        );
        assert_eq!(input, args(&["--model", "sonnet"]));
    }
    assert_eq!(launch::resume_args(id), args(&["--resume", id]));
    assert!(launch::runtime_args().is_empty());
    for input in [
        "--resume",
        "--resume=",
        "--resume=not-a-uuid",
        "--resume=--continue",
    ] {
        assert!(
            launch::take_adopted_session(&mut args(&[input])).is_err(),
            "{input}"
        );
    }
    assert_eq!(
        launch::take_adopted_session(&mut args(&["--resume", id, &format!("--resume={id}")]))
            .unwrap_err(),
        "Give at most one session to resume"
    );
}

#[test]
fn cursor_interactive_options_do_not_accept_trust_or_headless_aliases() {
    assert!(matches!(
        launch::supported_option("--model"),
        Some(LaunchOption::Value)
    ));
    assert!(matches!(
        launch::supported_option("--plan"),
        Some(LaunchOption::Flag)
    ));
    assert!(matches!(
        launch::supported_option("--mode"),
        Some(LaunchOption::Choice(["plan", "ask"]))
    ));
    for option in [
        "--force",
        "--yolo",
        "--trust",
        "--sandbox",
        "--approve-mcps",
        "-f",
        "-p",
        "worker",
        "--plugin-dir",
        "--continue",
    ] {
        assert!(launch::supported_option(option).is_none(), "{option}");
    }
}

#[test]
fn cursor_notice_keeps_four_hook_consent_and_cursor_adapter() {
    let notice = launch::setup_notice(Path::new("/project"));
    assert_eq!(notice.path, Path::new("/project/.cursor/hooks.json"));
    assert!(notice.message.starts_with("Add Bus-owned sessionStart, beforeSubmitPrompt, afterAgentResponse and stop observation hooks."));
    assert!(notice
        .message
        .contains("Complete normal Cursor trust/setup."));
    assert!(notice.message.ends_with("--bus-callback cursor-hook."));
}
