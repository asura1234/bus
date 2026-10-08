//! Codex-specific interactive argv and project setup metadata.
use super::super::launch::{is_session_uuid, LaunchOption, SetupNotice};
use std::path::Path;

/// Each launch hosts its own runtime so hooks inherit that launch's environment.
pub(crate) fn runtime_args() -> Vec<String> {
    vec!["--no-daemon".into()]
}

pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["resume".into(), session.into()]
}

/// Only a leading `resume ID` is a subcommand; later `resume` can be a value.
pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let mut found = None;
    while args.first().is_some_and(|arg| arg == "resume") {
        if found.is_some() {
            return Err("Give at most one session to resume".into());
        }
        args.remove(0);
        let value = if args.is_empty() {
            String::new()
        } else {
            args.remove(0)
        };
        if !is_session_uuid(&value) {
            return Err(format!(
                "Resume needs a session ID (a UUID), not {value:?}; pickers, --continue and --last are not supported"
            ));
        }
        found = Some(value);
    }
    Ok(found)
}

pub(crate) fn supported_option(name: &str) -> Option<LaunchOption> {
    use LaunchOption::{Choice, Flag, Value};
    match name {
        "--model" | "-m" => Some(Value),
        "--no-alt-screen" | "--strict-config" | "--oss" | "--search" => Some(Flag),
        "--local-provider" => Some(Choice(&["ollama", "lmstudio"])),
        _ => None,
    }
}

/// The caller resolves the project root before requesting the notice.
pub(crate) fn setup_notice(root: &Path) -> SetupNotice {
    SetupNotice {
        path: root.join(".codex/hooks.json"),
        message: "Add Bus-owned SessionStart, UserPromptSubmit and Stop observation hooks. Trust this project and all three definitions in Codex. Bus enables room delivery automatically when the terminal is ready. Codex emits SessionStart with the first room prompt; no priming message or restart is needed. Existing hooks/notify remain. Cleanup: remove only entries whose command is this Bus executable plus --bus-callback codex-hook.".into(),
    }
}
