//! Codex-specific interactive argv and project setup metadata.
use super::super::{
    launch::{argument_positions, is_session_uuid, SetupNotice},
    ProviderKind,
};
use std::path::Path;

/// Each launch hosts its own runtime so hooks inherit that launch's environment.
pub(crate) fn runtime_args() -> Vec<String> {
    vec![
        "--no-daemon".into(),
        "-c".into(),
        "check_for_update_on_startup=false".into(),
        // Draft transfers use these verified native actions. Codex rejects
        // colliding user bindings at startup before any composer can be edited.
        "-c".into(),
        "tui.keymap.global.open_external_editor=[\"ctrl-g\"]".into(),
        "-c".into(),
        "tui.keymap.editor.delete_backward=[\"backspace\"]".into(),
    ]
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
        message: super::super::launch::hook_notice_message("codex-hook"),
    }
}
