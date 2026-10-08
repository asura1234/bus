//! Cursor-specific interactive argv and project setup metadata.
use super::super::launch::{is_session_uuid, LaunchOption, SetupNotice};
use std::path::Path;

pub(crate) fn runtime_args() -> Vec<String> {
    Vec::new()
}

pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["--resume".into(), session.into()]
}

/// Cursor accepts `--resume ID` and `--resume=ID`, in any argument position.
pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let mut found = None;
    let mut index = 0;
    while index < args.len() {
        let attached = if args[index] == "--resume" {
            None
        } else if let Some(value) = args[index].strip_prefix("--resume=") {
            Some(value.to_owned())
        } else {
            index += 1;
            continue;
        };
        if found.is_some() {
            return Err("Give at most one session to resume".into());
        }
        args.remove(index);
        let value = match attached {
            Some(value) => value,
            None if index < args.len() => args.remove(index),
            None => String::new(),
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
        "--model" => Some(Value),
        "--plan" => Some(Flag),
        "--mode" => Some(Choice(&["plan", "ask"])),
        _ => None,
    }
}

/// The caller resolves the project root before requesting the notice.
pub(crate) fn setup_notice(root: &Path) -> SetupNotice {
    SetupNotice {
        path: root.join(".cursor/hooks.json"),
        message: "Add Bus-owned sessionStart, beforeSubmitPrompt, afterAgentResponse and stop observation hooks. Complete normal Cursor trust/setup. Bus enables room delivery automatically when the terminal is ready; queued prompts wait until then. Cleanup: remove only entries whose command is this Bus executable plus --bus-callback cursor-hook.".into(),
    }
}
