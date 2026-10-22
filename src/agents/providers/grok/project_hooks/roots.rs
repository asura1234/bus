//! The complete global-discovery boundary for the project hook transaction.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

// Resolve prospective paths through their existing ancestors. Existing broken
// aliases are an error, never evidence that a global source is unrelated.
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

fn absolute(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn configured_paths(base: &Path, home: &Path, cwd: &Path) -> Result<Vec<PathBuf>, String> {
    let file = match fs::File::open(base.join("config.toml")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut text = String::new();
    file.take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > 2 * 1024 * 1024 {
        return Err("Global Grok config is too large".into());
    }
    let config: toml::Value =
        toml::from_str(&text).map_err(|_| "Cannot parse global Grok config")?;
    let Some(entries) = config.get("hooks-paths") else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    for entry in entries
        .as_array()
        .ok_or("Global Grok hooks-paths must be an array")?
    {
        let text = entry
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Invalid global Grok hooks-paths entry")?;
        let path = if text == "~" {
            home.to_owned()
        } else if let Some(tail) = text.strip_prefix("~/") {
            home.join(tail)
        } else {
            let path = Path::new(text);
            if !path.is_absolute() {
                // Native config-relative semantics are not authenticated.
                // Protect both plausible bases, never the Bus server cwd.
                paths.push(canonical(&cwd.join(path))?);
            }
            absolute(path, base)
        };
        paths.push(canonical(&path)?);
    }
    Ok(paths)
}

pub(super) fn verify(file: &Path, cwd: &Path) -> Result<(), String> {
    let home = std::env::home_dir().ok_or("HOME is required to protect global Grok hooks")?;
    let home = canonical(&absolute(&home, cwd))?;
    let base = match std::env::var_os("GROK_HOME") {
        Some(path) if !path.is_empty() => canonical(&absolute(Path::new(&path), cwd))?,
        _ => canonical(&home.join(".grok"))?,
    };
    let mut roots = vec![base.clone()];
    let configured = configured_paths(&base, &home, cwd)?;
    // The built-in global source is <home>/hooks. Additional configured sources
    // may name a hook directory or a root containing hooks; protect both.
    let mut folders = vec![canonical(&base.join("hooks"))?];
    for path in configured {
        folders.push(path.clone());
        if !path.is_file() {
            folders.push(canonical(&path.join("hooks"))?);
        }
        roots.push(path);
    }
    roots.extend(folders.iter().cloned());
    let file = canonical(file)?;
    let folder = file.parent().ok_or("Missing project hook folder")?;
    if roots
        .iter()
        .any(|root| file.starts_with(root) || folder.starts_with(root))
    {
        return Err(
            "Project hooks overlap global Grok configuration; no capture authorized".into(),
        );
    }
    for folder in folders {
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) if folder.is_file() => continue,
            Err(e) => return Err(e.to_string()),
        };
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if canonical(&path)? == file {
                return Err(
                    "A global Grok hook resolves to the project hook; no capture authorized".into(),
                );
            }
        }
    }
    Ok(())
}
