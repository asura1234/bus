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
        message: "Add Bus-owned SessionStart, UserPromptSubmit and Stop observation hooks. Trust this project and all three definitions in Codex. Bus enables room delivery automatically when the terminal is ready. Codex emits SessionStart with the first room prompt; no priming message or restart is needed. Existing hooks/notify remain. Cleanup: remove only entries whose command is this Bus executable plus --bus-callback codex-hook.".into(),
    }
}
