use super::Mode;
use crate::agents::providers::{hook_json, launch::project_root};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
#[path = "dirs.rs"]
mod dirs;
#[path = "roots.rs"]
mod roots;

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    BeforeRename,
    AfterRename,
}

struct Target {
    cwd: PathBuf,
    cwd_dir: dirs::Dir,
    root_path: PathBuf,
    path: PathBuf,
    root: dirs::Dir,
    config: dirs::Dir,
    hooks: dirs::Dir,
}

struct PinnedRoot {
    cwd: PathBuf,
    cwd_dir: dirs::Dir,
    path: PathBuf,
    dir: dirs::Dir,
}

// Resolve the parent (e.g. macOS /var -> /private/var), but never follow the
// project directory itself. Keep that descriptor across executable validation.
fn nofollow_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or("Missing project parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    Ok(parent.join(path.file_name().ok_or("Missing project name")?))
}

impl PinnedRoot {
    fn open(project: &Path) -> Result<Self, String> {
        let project = nofollow_path(project)?;
        let cwd_dir = dirs::Dir::open(&project)?;
        let path = nofollow_path(&project_root(&project))?;
        if std::env::home_dir()
            .and_then(|p| p.canonicalize().ok())
            .is_some_and(|p| p == path)
        {
            return Err("Grok project hooks cannot modify HOME configuration".into());
        }
        let dir = dirs::Dir::open(&path)?;
        roots::verify(&path.join(".grok/hooks/bus.json"), &project)?;
        Ok(Self {
            cwd: project,
            cwd_dir,
            path,
            dir,
        })
    }

    fn target(self, mode: Mode) -> Result<Target, String> {
        if !self.dir.same(&dirs::Dir::open(&self.path)?)?
            || !self.cwd_dir.same(&dirs::Dir::open(&self.cwd)?)?
        {
            return Err("Project directory changed during validation".into());
        }
        let path = self.path.join(".grok/hooks/bus.json");
        roots::verify(&path, &self.cwd)?;
        let config = self.dir.child(".grok", mode == Mode::Install)?;
        let hooks = config.child("hooks", mode == Mode::Install)?;
        let target = Target {
            cwd: self.cwd,
            cwd_dir: self.cwd_dir,
            root_path: self.path,
            path,
            root: self.dir,
            config,
            hooks,
        };
        target.verify()?;
        Ok(target)
    }
}

impl Target {
    fn verify(&self) -> Result<(), String> {
        let root = dirs::Dir::open(&self.root_path)?;
        let config = root.child(".grok", false)?;
        let hooks = config.child("hooks", false)?;
        if !self.cwd_dir.same(&dirs::Dir::open(&self.cwd)?)?
            || !self.root.same(&root)?
            || !self.config.same(&config)?
            || !self.hooks.same(&hooks)?
        {
            return Err("Project hook directory identity changed; no capture authorized".into());
        }
        roots::verify(&self.path, &self.cwd)
    }
}

pub(super) fn update<T>(
    project: &Path,
    binary: &Path,
    mode: Mode,
    validate: impl FnOnce() -> Result<T, String>,
) -> Result<(PathBuf, bool, T), String> {
    transact(project, binary, mode, validate, |_| {})
}

// The callback lets tests replace directories at the exact syscall boundary;
// production always supplies a no-op, with the identical descriptor I/O.
fn transact<T>(
    project: &Path,
    binary: &Path,
    mode: Mode,
    validate: impl FnOnce() -> Result<T, String>,
    checkpoint: impl FnMut(Stage),
) -> Result<(PathBuf, bool, T), String> {
    let root = PinnedRoot::open(project)?;
    let validated = validate()?;
    let target = root.target(mode)?;
    let _lease = target.hooks.lock()?;
    target.verify()?;
    let previous = target.hooks.read()?;
    let document: Value = match previous.as_ref() {
        Some((_, bytes)) => {
            serde_json::from_slice(bytes).map_err(|_| "Invalid project hook JSON")?
        }
        None if mode == Mode::Install => json!({}),
        None => return Err("Bus resume hooks are missing; existing configuration was kept".into()),
    };
    if document["disableAllHooks"].as_bool() == Some(true) {
        return Err("Grok project hooks are disabled; existing configuration was kept".into());
    }
    let contract = crate::agents::providers::grok::HOOK_CONTRACT;
    if mode == Mode::Resume && !hook_json::has_owned_hooks(&document, contract) {
        return Err(
            "Bus resume hooks are missing or changed; existing configuration was kept".into(),
        );
    }
    let merged = hook_json::merge_hooks(document.clone(), contract, binary)?;
    if mode == Mode::Resume && document == merged {
        target.verify()?;
        return Ok((target.path, false, validated));
    }
    let bytes = serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?;
    let current = target.hooks.read()?;
    let unchanged = match (&previous, &current) {
        (Some((old, bytes)), Some((now, current))) => dirs::same(old, now)? && bytes == current,
        (None, None) => true,
        _ => false,
    };
    if !unchanged {
        return Err("Project hook file changed during setup; retry confirmation".into());
    }
    target.verify()?;
    let written = target.hooks.replace(&bytes, checkpoint)?;
    // No caller may initialize capture until both the directory chain and the
    // committed leaf are the ones checked/written through our pinned handles.
    target.verify()?;
    let (current, contents) = target
        .hooks
        .read()?
        .ok_or("Project hook disappeared after write")?;
    if !dirs::same(&written, &current)? || contents != bytes {
        return Err("Project hook identity changed after write; no capture authorized".into());
    }
    Ok((target.path, mode == Mode::Resume, validated))
}

#[cfg(test)]
#[path = "../tests/project_hooks_test.rs"]
mod tests;
