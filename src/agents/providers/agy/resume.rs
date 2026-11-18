//! Cold-resume validation and capture argument reconstruction for AGY workers.
use super::{project_hooks, statusline};
use crate::agents::providers::hook_json::HookContext;
use std::path::{Path, PathBuf};

pub(crate) fn capture_args(
    hooks: &HookContext,
    project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    let (hooks_path, hooks_rebound) = project_hooks::rebind(project, &hooks.binary)?;
    let (settings_path, settings_rebound) = statusline::rebind(&hooks.spool, &hooks.binary)?;
    let args = vec![
        "--settings".into(),
        settings_path.to_string_lossy().into_owned(),
    ];
    Ok((hooks_path, args, hooks_rebound || settings_rebound))
}
