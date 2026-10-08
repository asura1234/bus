//! Reconstruct capture only for an attested Cursor launch.

use std::path::{Path, PathBuf};

use super::super::{
    hook_json::{resume_hooks, HookContext, HookContract},
    ProviderKind,
};

pub(crate) fn capture_args(
    hooks: &HookContext,
    project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    let path = project.join(".cursor/hooks.json");
    let rebound = resume_hooks(
        &path,
        HookContract::for_provider(ProviderKind::Cursor),
        &hooks.binary,
    )?;
    Ok((path, Vec::new(), rebound))
}
