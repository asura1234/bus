//! Lane A+D replaces these explicit G0 launch refusals.
use crate::agents::providers::{
    hook_json::HookContext,
    launch::{LaunchSpec, PreparedLaunch, SetupNotice},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
const INCOMPLETE: &str = "Grok launch support is not implemented (G0; lane A+D)";
pub(crate) fn runtime_args() -> Vec<String> {
    vec![
        "--no-leader".into(),
        "--fullscreen".into(),
        "--no-auto-update".into(),
    ]
}
pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["--resume".into(), session.into()]
}
pub(crate) fn take_adopted_session(_args: &mut Vec<String>) -> Result<Option<String>, String> {
    Err(INCOMPLETE.into())
}
pub(crate) fn option_arity(_name: &str) -> Option<usize> {
    None
}
pub(crate) fn is_session_selector(name: &str) -> bool {
    matches!(
        name,
        "--resume" | "-r" | "--continue" | "-c" | "--session-id" | "-s" | "--fork-session"
    )
}
pub(crate) fn validate_args(_args: &[String]) -> Result<(), String> {
    Err(INCOMPLETE.into())
}
pub(crate) fn resolve_executable() -> Result<PathBuf, String> {
    Err(INCOMPLETE.into())
}
pub(crate) fn environment(hooks: &HookContext) -> Result<HashMap<String, String>, String> {
    resolve_executable().map(|_| hooks.env())
}
pub(crate) fn setup_notice(root: &Path) -> SetupNotice {
    SetupNotice {
        path: root.join(".grok/hooks/bus.json"),
        message: crate::agents::providers::launch::hook_notice_message("grok-hook"),
    }
}
pub(crate) fn prepare(
    _spec: &LaunchSpec,
    _initialize: impl FnOnce(&HookContext) -> Result<(), String>,
) -> Result<PreparedLaunch, String> {
    Err(INCOMPLETE.into())
}
