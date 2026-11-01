//! Reconstruct capture for the validated Codex launch without daemon routing.

use std::path::{Path, PathBuf};

use super::super::{
    hook_json::{self, resume_hooks, HookContext, HookContract},
    ProviderKind,
};

pub(crate) fn capture_args(
    hooks: &HookContext,
    project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    super::launch::validate_project_trust(project)?;
    let path = project.join(".codex/hooks.json");
    let binary = hook_json::codex_resume_hook_binary(hooks)?;
    let rebound = resume_hooks(
        &path,
        HookContract::for_provider(ProviderKind::Codex),
        &binary,
    )?;
    // The shared app-server daemon would inherit a different pane's hook env.
    Ok((path, super::launch::runtime_args(), rebound))
}
