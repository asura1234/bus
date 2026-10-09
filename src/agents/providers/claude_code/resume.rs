//! Reconstruct only the already-validated launch's Claude capture arguments.

use std::path::{Path, PathBuf};

use super::super::{
    hook_json::{resume_hooks, HookContext, HookContract},
    ProviderKind,
};
use super::statusline;

pub(crate) fn capture_args(
    hooks: &HookContext,
    _project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    let path = hooks.spool.join("claude-settings.json");
    let rebound = resume_hooks(
        &path,
        HookContract::for_provider(ProviderKind::ClaudeCode),
        &hooks.binary,
    )?;
    let rebound =
        statusline::rebind(&hooks.spool, &hooks.binary).map_err(|e| e.to_string())? || rebound;
    let args = vec!["--settings".into(), path.to_string_lossy().into_owned()];
    Ok((path, args, rebound))
}
