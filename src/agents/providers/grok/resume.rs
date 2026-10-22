//! Rebuild only existing, coordinator-attested Grok capture; never install missing hooks.
use crate::agents::providers::hook_json::{self, HookContext};
use std::path::{Path, PathBuf};

pub(crate) fn capture_args(
    hooks: &HookContext,
    project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    let path = super::launch::project_hook_path(project)?;
    super::launch::saved_executable(hooks)?;
    // Missing saved options are an invalid Grok capture, not an empty launch.
    let options: Vec<String> = serde_json::from_value(hook_json::read_resume_json(
        &hooks.spool.join("launch-options.json"),
    )?)
    .map_err(|_| "Saved Grok launch options are invalid")?;
    super::launch::validate_args(&options)?;
    let document = hook_json::read_resume_json(&path)?;
    if document["disableAllHooks"].as_bool() == Some(true) {
        return Err("Bus resume hooks are disabled; existing configuration was kept".into());
    }
    let contract = super::HOOK_CONTRACT;
    let command = contract.command(&hooks.binary);
    if hook_json::validate_resume_hooks(&path, contract, &hooks.binary).is_ok()
        && !contract.events.iter().all(|event| {
            document["hooks"][event]
                .as_array()
                .is_some_and(|entries| entries.contains(&contract.entry(&command)))
        })
    {
        return Err("Bus resume hooks are changed; existing configuration was kept".into());
    }
    let rebound = hook_json::resume_hooks(&path, contract, &hooks.binary)?;
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
