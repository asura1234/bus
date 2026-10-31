//! Cold resume verifies the private Kimi installation and retained hooks.
use crate::agents::providers::hook_json::HookContext;
use std::path::{Path, PathBuf};

pub(crate) fn capture_args(
    hooks: &HookContext,
    _project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    let home = super::launch::saved_home(hooks)?;
    super::launch::saved_executable(hooks)?;
    let path = home.join("config.toml");
    let rebound = super::launch::rebind_hooks(hooks)?;
    Ok((path, Vec::new(), rebound))
}
