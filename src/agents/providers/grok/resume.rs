//! Rebuild only existing, coordinator-attested Grok capture; never install missing hooks.
use crate::agents::providers::hook_json::{self, HookContext};
use std::path::{Path, PathBuf};

pub(crate) fn capture_args(
    hooks: &HookContext,
    project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    // Missing saved options are an invalid Grok capture, not an empty launch.
    let options: Vec<String> = serde_json::from_value(hook_json::read_resume_json(
        &hooks.spool.join("launch-options.json"),
    )?)
    .map_err(|_| "Saved Grok launch options are invalid")?;
    super::launch::validate_args(&options)?;
    let home = super::launch::saved_home(hooks)?;
    let (path, rebound, _, _) = super::project_hooks::update(
        project,
        &hooks.binary,
        super::project_hooks::Mode::Resume,
        Some(&home),
        || super::launch::saved_executable(hooks),
    )?;
    // Options are loaded separately by the shared resume owner. Return only
    // missing generated flags so it cannot duplicate explicit user flags.
    Ok((
        path,
        super::launch::runtime_additions(&super::launch::resumable_options(&options)),
        rebound,
    ))
}

#[cfg(test)]
#[path = "tests/resume_test.rs"]
mod resume_test;
