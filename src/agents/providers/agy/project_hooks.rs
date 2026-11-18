//! Named-block hook installation and rebinding for `<project_root>/.agents/hooks.json`.
//!
//! AGY discovers workspace hooks from `.agents/hooks.json` using top-level named
//! hook groups (`{"bus": {"SessionStart": [...], "PreInvocation": [...], "Stop": [...]}}`)
//! with flat command handler arrays for lifecycle events.
use crate::agents::providers::{
    grok::project_hooks::ensure_git_exclude as ensure_shared_exclude,
    hook_json::{self, HookContract},
    ProviderKind,
};
use crate::platform::fs::{guarded_atomic_write, guarded_lock_file, private_dir};
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) const HOOK_EVENTS: [&str; 3] = ["SessionStart", "PreInvocation", "Stop"];

pub(crate) fn hooks_path(project_root: &Path) -> PathBuf {
    project_root.join(".agents/hooks.json")
}

fn contract() -> HookContract {
    HookContract::for_provider(ProviderKind::Agy)
}

fn parse_document(bytes: &[u8]) -> Result<Value, String> {
    let trimmed = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    serde_json::from_slice(trimmed).map_err(|e| format!("Invalid .agents/hooks.json: {e}"))
}

#[cfg(test)]
pub(in crate::agents::providers) fn has_owned_hooks(document: &Value) -> bool {
    hook_json::has_owned_hooks(document, contract())
}

pub(crate) fn hooks_are_current(path: &Path, binary: &Path) -> bool {
    hook_json::hooks_are_current(path, contract(), binary)
}

#[cfg(test)]
fn assert_temporary(path: &Path) {
    let temp = std::env::temp_dir();
    let canonical = temp.canonicalize().unwrap_or_else(|_| temp.clone());
    assert!(
        path.starts_with(&temp) || path.starts_with(&canonical),
        "tests must not install AGY hooks outside the temp directory: {}",
        path.display()
    );
}

pub(crate) fn install(project_root: &Path, binary: &Path) -> Result<PathBuf, String> {
    let path = hooks_path(project_root);
    #[cfg(test)]
    assert_temporary(&path);
    let parent = path.parent().ok_or("Missing .agents parent directory")?;
    private_dir(parent).map_err(|e| e.to_string())?;
    let _lease = guarded_lock_file(&parent.join(".bus-hooks.lock")).map_err(|e| e.to_string())?;
    let previous = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let created = previous.is_none();
    let document = match previous.as_deref() {
        Some(bytes) => parse_document(bytes)?,
        None => json!({}),
    };
    let merged = hook_json::merge_hooks(document.clone(), contract(), binary)?;
    if previous.is_some() && merged == document {
        ensure_git_exclude(project_root, false);
        return Ok(path);
    }
    let current = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    if current != previous {
        return Err("AGY hook config changed during setup; retry confirmation".into());
    }
    let formatted = serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?;
    guarded_atomic_write(&path, &formatted).map_err(|e| e.to_string())?;
    ensure_git_exclude(project_root, created);
    Ok(path)
}

pub(crate) fn rebind(project_root: &Path, binary: &Path) -> Result<(PathBuf, bool), String> {
    let path = hooks_path(project_root);
    #[cfg(test)]
    assert_temporary(&path);
    let parent = path.parent().ok_or("Missing .agents parent directory")?;
    if !parent.is_dir() {
        return Err("Bus AGY resume hooks are missing; existing configuration was kept".into());
    }
    let _lease = guarded_lock_file(&parent.join(".bus-hooks.lock")).map_err(|e| e.to_string())?;
    let previous = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err("Bus AGY resume hooks are missing; existing configuration was kept".into());
        }
        Err(e) => {
            tracing::warn!(
                event = "bus.resume.hooks_unreadable",
                provider = "agy",
                error_kind = ?e.kind(),
                "Cannot read AGY project hooks during cold resume"
            );
            return Err(format!(
                "Bus AGY resume hooks cannot be read ({e}); existing configuration was kept"
            ));
        }
    };
    let document = parse_document(&previous).inspect_err(|error| {
        tracing::warn!(
            event = "bus.resume.hooks_invalid",
            provider = "agy",
            %error,
            "Invalid AGY project hooks JSON during cold resume"
        );
    })?;
    if !hook_json::has_owned_hooks(&document, contract()) {
        return Err(
            "Bus AGY resume hooks are missing or changed; existing configuration was kept".into(),
        );
    }
    let merged = hook_json::merge_hooks(document.clone(), contract(), binary)?;
    if merged == document {
        return Ok((path, false));
    }
    let current = fs::read(&path).map_err(|e| e.to_string())?;
    if current != previous {
        return Err("AGY hook config changed during resume rebind".into());
    }
    let formatted = serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?;
    guarded_atomic_write(&path, &formatted).map_err(|e| e.to_string())?;
    ensure_git_exclude(project_root, false);
    Ok((path, true))
}

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
    if let Err(error) = ensure_shared_exclude(project_root, &agy_home, entries) {
        tracing::warn!(
            root = %project_root.display(),
            %error,
            "Could not exclude Bus-generated AGY hook artifacts from Git status"
        );
    }
}
