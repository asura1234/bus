//! Cursor-specific interactive argv and project setup metadata.
use super::super::{
    launch::{take_resume_option, SetupNotice},
    ProviderKind,
};
use std::path::Path;

pub(crate) fn runtime_args() -> Vec<String> {
    Vec::new()
}

pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["--resume".into(), session.into()]
}

/// Cursor accepts `--resume ID` and `--resume=ID`, in any argument position.
pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    Ok(take_resume_option(ProviderKind::Cursor, args))
}

/// The caller resolves the project root before requesting the notice.
pub(crate) fn setup_notice(root: &Path) -> SetupNotice {
    SetupNotice {
        path: root.join(".cursor/hooks.json"),
        message: "Bus will add a few hooks to this project's Cursor settings, so room messages can reach this agent and its replies can reach the room. Complete Cursor's usual trust and setup prompts. Messages you send before it is ready wait and are delivered once it is. Your existing hooks stay as they are. To undo, delete the entries that run this Bus executable with --bus-callback cursor-hook.".into(),
    }
}
