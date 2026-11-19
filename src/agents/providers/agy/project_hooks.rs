//! Named-block hook contracts and Git exclusions for AGY workspace capture.
use crate::agents::providers::{
    hook_json::{self, HookContract},
    ProviderKind,
};
#[cfg(test)]
use serde_json::Value;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

pub(crate) const HOOK_EVENTS: [&str; 3] = ["SessionStart", "PreInvocation", "Stop"];
pub(crate) fn hooks_path(project_root: &Path) -> PathBuf {
    project_root.join(".agents/hooks.json")
}
fn contract() -> HookContract {
    HookContract::for_provider(ProviderKind::Agy)
}
#[cfg(test)]
pub(in crate::agents::providers) fn has_owned_hooks(document: &Value) -> bool {
    hook_json::has_owned_hooks(document, contract())
}
pub(crate) fn hooks_are_current(path: &Path, binary: &Path) -> bool {
    hook_json::hooks_are_current(path, contract(), binary)
}
pub(crate) fn install(project_root: &Path, binary: &Path) -> Result<PathBuf, String> {
    let path = hooks_path(project_root);
    let update = hook_json::install_hooks_with_status(&path, contract(), binary)?;
    ensure_git_exclude(project_root, update.created);
    Ok(path)
}
pub(crate) fn rebind(project_root: &Path, binary: &Path) -> Result<(PathBuf, bool), String> {
    let path = hooks_path(project_root);
    let changed = hook_json::rebind_hooks_with_status(&path, contract(), binary)
        .map_err(|error| format!("Bus AGY resume hooks are missing or changed; existing configuration was kept ({error})"))?;
    if changed {
        ensure_git_exclude(project_root, false);
    }
    Ok((path, changed))
}
#[cfg(not(unix))]
fn ensure_git_exclude(_project_root: &Path, _created_hooks_file: bool) {}

#[cfg(unix)]
fn ensure_git_exclude(project_root: &Path, created_hooks_file: bool) {
    let Some(home) = std::env::home_dir() else {
        return;
    };
    let tracked = Command::new("git")
        .current_dir(project_root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["ls-files", "--error-unmatch", ".agents/hooks.json"])
        .output()
        .is_ok_and(|out| out.status.success());
    let entries: &[&str] = if created_hooks_file && !tracked {
        &["/.agents/hooks.json", "/.agents/.bus-hooks.lock"]
    } else {
        &["/.agents/.bus-hooks.lock"]
    };
    let agy_home = home.canonicalize().unwrap_or(home).join(".gemini");
    let protected = [agy_home.clone(), agy_home.join("antigravity-cli")];
    if let Err(error) =
        super::super::git_exclude::ensure(project_root, entries, |directory, leaf| {
            super::super::git_exclude::verify_sources(directory, protected.iter().cloned(), leaf)
        })
    {
        tracing::warn!(
            root = %project_root.display(),
            %error,
            "Could not exclude Bus-generated AGY hook artifacts from Git status"
        );
    }
}
