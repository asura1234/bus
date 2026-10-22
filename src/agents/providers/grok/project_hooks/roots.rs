//! The complete global-discovery boundary for the project hook transaction.
use super::dirs::Dir;
use std::{
    fs,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

// Resolve prospective paths through existing ancestors. Broken aliases refuse.
pub(super) fn canonical(path: &Path) -> Result<PathBuf, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => path
            .canonicalize()
            .map_err(|e| format!("Cannot resolve Grok hook source: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("Grok hook source has no parent")?;
            let name = path.file_name().ok_or("Grok hook source has no filename")?;
            Ok(canonical(parent)?.join(name))
        }
        Err(e) => Err(e.to_string()),
    }
}

pub(super) fn home(cwd: &Path, saved: Option<&Path>) -> Result<PathBuf, String> {
    let path = saved
        .map(Path::to_owned)
        .or_else(|| {
            std::env::var_os("GROK_HOME")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| std::env::home_dir().map(|p| p.join(".grok")))
        .ok_or("HOME is required to protect global Grok hooks")?;
    canonical(&if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

fn configured_paths(base: &Path) -> Result<Vec<PathBuf>, String> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(base.join("hooks-paths"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Global Grok hooks-paths is not a regular file".into());
    }
    let mut text = String::new();
    file.take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > 2 * 1024 * 1024 {
        return Err("Global Grok hooks-paths is too large".into());
    }
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Path::new)
        .filter(|p| p.is_absolute())
        .map(|p| {
            p.canonicalize()
                .map_err(|e| format!("Cannot resolve registered global Grok hook source: {e}"))
        })
        .collect()
}

fn existing_directory(path: &Path) -> Result<Option<Dir>, String> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Dir::open(&canonical(path)?).map(Some),
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
fn reject_shared(path: &Path, leaf: Option<&fs::File>) -> Result<(), String> {
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
    if super::dirs::same(&entry, leaf)? {
        return Err(
            "A global Grok hook resolves to the project hook; no capture authorized".into(),
        );
    }
    Ok(())
}

pub(super) fn verify(root: &Dir, folder: Option<&Dir>, base: &Path) -> Result<(), String> {
    let home = std::env::home_dir().ok_or("HOME is required to protect global Grok hooks")?;
    if let Some(home) = existing_directory(&home)? {
        if root.same(&home)? {
            return Err("Grok project hooks cannot modify HOME configuration".into());
        }
    }
    let configured = configured_paths(base)?;
    let mut sources = vec![(base.to_owned(), false), (base.join("hooks"), true)];
    sources.extend(configured.iter().cloned().map(|p| (p, true)));
    // Protect registered directories and their hooks subfolder; registered
    // regular files also participate in the leaf identity comparison below.
    sources.extend(
        configured
            .iter()
            .map(|p| p.join("hooks"))
            .filter(|p| p.is_dir())
            .map(|p| (p, true)),
    );
    let target = folder.unwrap_or(root);
    let leaf = match folder {
        Some(folder) => folder.leaf()?,
        None => None,
    };
    for (source, scan) in sources {
        if let Some(global) = existing_directory(&source)? {
            if target.within(&global)? {
                return Err(
                    "Project hooks overlap global Grok configuration; no capture authorized".into(),
                );
            }
            if !scan {
                continue;
            }
            for entry in fs::read_dir(&source).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                // Missing/broken global aliases fail closed before creation.
                let resolved = canonical(&path)?;
                reject_shared(&resolved, leaf.as_ref())?;
            }
        } else {
            reject_shared(&source, leaf.as_ref())?;
        }
    }
    Ok(())
}
