//! Append only caller-owned Git exclusions under pinned descriptor and alias checks.
use std::{
    ffi::OsStr,
    fs,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn ensure(
    root: &Path,
    entries: &[&str],
    verify: impl Fn(&super::hook_directory::Dir, Option<&fs::File>) -> Result<(), String>,
) -> Result<(), String> {
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
    let directory = super::hook_directory::Dir::open(&info)?;
    verify(&directory, None)?;
    let mut file = directory.open_at(
        OsStr::new("exclude"),
        libc::O_RDWR | libc::O_APPEND | libc::O_CREAT,
    )?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err("Git exclude is not an unshared ordinary file".into());
    }
    // Both appenders serialize the read/check/write as one operation. The lock
    // releases with the file even if a later read or write fails.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    verify(&directory, Some(&file))?;
    if file.metadata().map_err(|e| e.to_string())?.nlink() != 1 {
        return Err("Git exclude became shared".into());
    }
    let mut existing = String::new();
    file.read_to_string(&mut existing)
        .map_err(|e| e.to_string())?;
    let missing: Vec<_> = entries
        .iter()
        .copied()
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

// Resolve prospective paths through existing ancestors. Broken aliases refuse.
pub(in crate::agents::providers) fn canonical(path: &Path) -> Result<PathBuf, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => path
            .canonicalize()
            .map_err(|e| format!("Cannot resolve provider configuration source: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or("provider configuration source has no parent")?;
            let name = path
                .file_name()
                .ok_or("provider configuration source has no filename")?;
            Ok(canonical(parent)?.join(name))
        }
        Err(e) => Err(e.to_string()),
    }
}

pub(in crate::agents::providers) fn existing_directory(
    path: &Path,
) -> Result<Option<super::hook_directory::Dir>, String> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            super::hook_directory::Dir::open(&canonical(path)?).map(Some)
        }
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Distinguish a missing source from a broken existing symlink.
            canonical(path)?;
            Ok(None)
        }
        Err(e) => Err(e.to_string()),
    }
}

// Compare opened objects, not resolved spellings (APFS may retain both names).
pub(in crate::agents::providers) fn reject_shared(
    path: &Path,
    leaf: Option<&fs::File>,
) -> Result<(), String> {
    let Some(leaf) = leaf else {
        return Ok(());
    };
    let entry = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(entry) => entry,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    if super::hook_directory::same(&entry, leaf)? {
        return Err("Global provider configuration aliases the protected file".into());
    }
    Ok(())
}

pub(super) fn verify_sources(
    target: &super::hook_directory::Dir,
    sources: impl IntoIterator<Item = PathBuf>,
    leaf: Option<&fs::File>,
) -> Result<(), String> {
    for source in sources {
        if let Some(directory) = existing_directory(&source)? {
            if target.within(&directory)? {
                return Err("Git exclude overlaps global provider configuration".into());
            }
            // Resolve even without an opened leaf. A dangling global alias
            // must fail closed before creating the exclude it could target.
            for entry in fs::read_dir(&source).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                reject_shared(&canonical(&path)?, leaf)?;
            }
        } else {
            reject_shared(&source, leaf)?;
        }
    }
    Ok(())
}
