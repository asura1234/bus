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
        message: super::super::launch::hook_notice_message("cursor-hook"),
    }
}
