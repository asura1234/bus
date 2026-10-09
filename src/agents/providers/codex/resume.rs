//! Reconstruct capture for the validated Codex launch without daemon routing.

use std::path::{Path, PathBuf};

use super::super::{
    hook_json::{resume_hooks, HookContext, HookContract},
    ProviderKind,
};

pub(crate) fn capture_args(
    hooks: &HookContext,
    project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    let path = project.join(".codex/hooks.json");
    let rebound = resume_hooks(
        &path,
        HookContract::for_provider(ProviderKind::Codex),
        &hooks.binary,
    )?;
    // The shared app-server daemon would inherit a different pane's hook env.
    Ok((path, vec!["--no-daemon".into()], rebound))
}
