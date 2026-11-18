//! Named-block hook installation and rebinding for `<project_root>/.agents/hooks.json`.
//!
//! AGY discovers workspace hooks from `.agents/hooks.json` using top-level named
//! hook groups (`{"bus": {"SessionStart": [...], "PreInvocation": [...], "Stop": [...]}}`)
//! with flat command handler arrays for lifecycle events.
use crate::platform::fs::{guarded_atomic_write, guarded_lock_file, private_dir};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) const HOOK_EVENTS: [&str; 3] = ["SessionStart", "PreInvocation", "Stop"];
const ADAPTER: &str = "agy-hook";
const BLOCK_NAME: &str = "bus";

pub(crate) fn hooks_path(project_root: &Path) -> PathBuf {
    project_root.join(".agents/hooks.json")
}

pub(crate) fn command_for(binary: &Path) -> String {
    let quoted = crate::platform::remote_reattach_program(&binary.to_string_lossy());
    format!("{quoted} --bus-callback {ADAPTER}")
}

fn entry_for(command: &str) -> Value {
    json!({
        "type": "command",
        "command": command,
        "timeout": 5,
    })
}

fn is_owned_entry(entry: &Value) -> bool {
    let Some(command) = entry.get("command").and_then(Value::as_str) else {
        return false;
    };
    let Some(program) = command.strip_suffix(&format!(" --bus-callback {ADAPTER}")) else {
        return false;
    };
    let path = match program
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        Some(quoted) => quoted.replace("'\\''", "'"),
        None => program.to_owned(),
    };
    !path.is_empty()
        && crate::platform::remote_reattach_program(&path) == program
        && *entry == entry_for(command)
}

fn parse_document(bytes: &[u8]) -> Result<Value, String> {
    let trimmed = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    serde_json::from_slice(trimmed).map_err(|e| format!("Invalid .agents/hooks.json: {e}"))
}

pub(crate) fn merge_hooks(mut document: Value, binary: &Path) -> Result<Value, String> {
    let root = document
        .as_object_mut()
        .ok_or("AGY hook config must be a JSON object")?;
    let block = root
        .entry(BLOCK_NAME)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("AGY 'bus' hook group must be a JSON object")?;
    if block.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Err(
            "AGY 'bus' hook group is explicitly disabled in .agents/hooks.json; enable or remove it before setup"
                .into(),
        );
    }
    let command = command_for(binary);
    let owned = entry_for(&command);
    for event in HOOK_EVENTS {
        let group = block
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("AGY hook event {event} must contain an array"))?;
        let mut placed = false;
        group.retain_mut(|entry| {
            if !is_owned_entry(entry) {
                return true;
            }
            if placed {
                return false;
            }
            placed = true;
            *entry = owned.clone();
            true
        });
        if placed {
            continue;
        }
        if group
            .iter()
            .any(|entry| entry.to_string().contains("--bus-callback"))
        {
            return Err(format!(
                "Conflicting Bus hook in .agents/hooks.json {event}; inspect/remove that owned entry before setup"
            ));
        }
        group.push(owned.clone());
    }
    Ok(document)
}

pub(crate) fn has_owned_hooks(document: &Value) -> bool {
    let Some(block) = document.get(BLOCK_NAME).and_then(Value::as_object) else {
        return false;
    };
    if block.get("enabled").and_then(Value::as_bool) == Some(false) {
        return false;
    }
    HOOK_EVENTS.iter().all(|event| {
        block
            .get(*event)
            .and_then(Value::as_array)
            .is_some_and(|group| group.iter().any(is_owned_entry))
    })
}

pub(crate) fn hooks_are_current(path: &Path, binary: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let Ok(document) = parse_document(&bytes) else {
        return false;
    };
    merge_hooks(document.clone(), binary).is_ok_and(|merged| merged == document)
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
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let created = previous.is_none();
    let document = match previous.as_deref() {
        Some(bytes) => parse_document(bytes)?,
        None => json!({}),
    };
    let merged = merge_hooks(document.clone(), binary)?;
    if previous.is_some() && merged == document {
        ensure_git_exclude(project_root, false);
        return Ok(path);
    }
    let current = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
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
    let previous = fs::read(&path)
        .map_err(|_| "Bus AGY resume hooks are missing; existing configuration was kept")?;
    let document = parse_document(&previous)?;
    if !has_owned_hooks(&document) {
        return Err(
            "Bus AGY resume hooks are missing or changed; existing configuration was kept".into(),
        );
    }
    let merged = merge_hooks(document.clone(), binary)?;
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
    if let Err(error) = try_ensure_git_exclude(project_root, created_hooks_file) {
        tracing::warn!(
            root = %project_root.display(),
            %error,
            "Could not exclude Bus-generated AGY hook artifacts from Git status"
        );
    }
}

fn try_ensure_git_exclude(project_root: &Path, created_hooks_file: bool) -> Result<(), String> {
    let output = Command::new("git")
        .current_dir(project_root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ])
        .output();
    let Ok(output) = output else { return Ok(()) };
    if !output.status.success() {
        return Ok(());
    }
    let text = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let mut lines = text.lines();
    let top = PathBuf::from(lines.next().ok_or("Git did not report its worktree root")?);
    let common = PathBuf::from(
        lines
            .next()
            .ok_or("Git did not report its common directory")?,
    );
    if lines.next().is_some()
        || top.canonicalize().map_err(|e| e.to_string())?
            != project_root.canonicalize().map_err(|e| e.to_string())?
        || !common.is_absolute()
    {
        return Ok(());
    }
    let tracked = Command::new("git")
        .current_dir(project_root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["ls-files", "--error-unmatch", ".agents/hooks.json"])
        .output()
        .is_ok_and(|out| out.status.success());
    let mut wanted = vec!["/.agents/.bus-hooks.lock"];
    if created_hooks_file && !tracked {
        wanted.insert(0, "/.agents/hooks.json");
    }
    let info = common.canonicalize().map_err(|e| e.to_string())?.join("info");
    if !info.is_dir()
        || fs::symlink_metadata(&info)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Ok(());
    }
    let exclude_path = info.join("exclude");
    if exclude_path.exists()
        && fs::symlink_metadata(&exclude_path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err("Git exclude is a symlink".into());
    }
    let _lock = guarded_lock_file(&info.join(".bus-exclude.lock")).map_err(|e| e.to_string())?;
    let mut existing = String::new();
    if exclude_path.exists() {
        fs::File::open(&exclude_path)
            .map_err(|e| e.to_string())?
            .read_to_string(&mut existing)
            .map_err(|e| e.to_string())?;
    }
    let missing: Vec<&str> = wanted
        .into_iter()
        .filter(|entry| !existing.lines().any(|line| line.trim() == *entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)
        .map_err(|e| e.to_string())?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        file.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    for entry in missing {
        file.write_all(entry.as_bytes()).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}
