//! Codex-specific interactive argv and project setup metadata.
use super::super::{
    launch::{argument_positions, is_session_uuid, SetupNotice},
    ProviderKind,
};
use std::path::Path;

/// Each launch hosts its own runtime so hooks inherit that launch's environment.
pub(crate) fn runtime_args() -> Vec<String> {
    vec!["--no-daemon".into()]
}

pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["resume".into(), session.into()]
}

/// Options can precede the command; option values named `resume` are literal.
pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let Some(index) = argument_positions(ProviderKind::Codex, args)
        .into_iter()
        .find(|&index| !args[index].starts_with('-'))
    else {
        return Ok(None);
    };
    if args[index] != "resume" {
        return Ok(None);
    }
    let Some(value) = args
        .get(index + 1)
        .filter(|value| is_session_uuid(value))
        .cloned()
    else {
        return Ok(None);
    };
    args.drain(index..index + 2);
    Ok(Some(value))
}

/// The caller resolves the project root before requesting the notice.
pub(crate) fn setup_notice(root: &Path) -> SetupNotice {
    SetupNotice {
        path: root.join(".codex/hooks.json"),
        message: "Bus will add a few hooks to this project's Codex settings, so room messages can reach this agent and its replies can reach the room. Codex will then ask you to trust this project and the new hooks; accept them. Your existing hooks stay as they are. To undo, delete the entries that run this Bus executable with --bus-callback codex-hook.".into(),
    }
}
