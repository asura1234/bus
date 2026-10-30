//! Keep only Bus-owned Grok hook artifacts out of a repository's Git status.
//! The user remains free to track other `.grok` settings and hooks.
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    process::Command,
};

const ENTRIES: [&str; 2] = ["/.grok/hooks/bus.json", "/.grok/hooks/.bus-hooks.lock"];

pub(super) fn ensure(root: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .current_dir(root)
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
            != root.canonicalize().map_err(|e| e.to_string())?
        || !common.is_absolute()
    {
        return Err("Git did not report this project as its worktree root".into());
    }
    let common = common.canonicalize().map_err(|e| e.to_string())?;
    let info = common.join("info");
    let metadata = fs::symlink_metadata(&info).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Git info directory is not an ordinary directory".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(info.join("exclude"))
        .map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Git exclude is not an ordinary file".into());
    }
    // Both appenders serialize the read/check/write as one operation. The lock
    // releases with the file even if a later read or write fails.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut existing = String::new();
    file.read_to_string(&mut existing)
        .map_err(|e| e.to_string())?;
    let missing: Vec<_> = ENTRIES
        .into_iter()
        .filter(|entry| !existing.lines().any(|line| line.trim() == *entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    if !existing.is_empty() && !existing.ends_with('\n') {
        file.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    for entry in missing {
        file.write_all(entry.as_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    Ok(())
}
