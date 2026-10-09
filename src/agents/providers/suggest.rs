//! Filesystem suggestions without UI or room state.

use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PathSuggestion {
    pub(crate) path: PathBuf,
    pub(crate) is_directory: bool,
}

pub(crate) fn suggestions(
    input: &str,
    directories_only: bool,
) -> Result<Vec<PathSuggestion>, String> {
    let expanded = if let Some(rest) = input.strip_prefix("~/") {
        std::env::home_dir()
            .ok_or("Home directory unavailable")?
            .join(rest)
    } else {
        PathBuf::from(input)
    };
    if !expanded.is_absolute() {
        return Ok(Vec::new());
    }
    let (parent, prefix) = if input.ends_with('/') {
        (expanded.as_path(), String::new())
    } else {
        (
            expanded.parent().ok_or("Missing directory")?,
            expanded
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        )
    };
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(parent).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        let is_directory = metadata.is_dir();
        if is_directory || (!directories_only && metadata.is_file()) {
            paths.push(PathSuggestion { path, is_directory });
        }
        if paths.len() == 200 {
            break;
        }
    }
    paths.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(paths)
}
