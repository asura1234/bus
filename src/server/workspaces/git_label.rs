use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSpaceMetadata {
    pub key: String,
    pub repo_name: String,
    pub repo_root: PathBuf,
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktreeInfo {
    pub repo_root: PathBuf,
    pub git_dir: PathBuf,
    pub git_common_dir: PathBuf,
    pub is_bare: bool,
    pub is_linked_worktree: bool,
}

pub fn fallback_label_from_cwd(cwd: &Path) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home = Path::new(&home);
        if cwd == home {
            return "~".to_string();
        }
    }

    cwd.file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| cwd.display().to_string())
}

pub fn git_worktree_info(cwd: &Path) -> Option<GitWorktreeInfo> {
    let repo_root = git_repo_root(cwd)?;
    let git_dir = canonicalize_best_effort_path(&git_dir_for_repo_root(&repo_root)?);
    let git_common_dir = canonicalize_best_effort_path(&git_common_dir_for_git_dir(&git_dir));
    let is_linked_worktree = git_dir != git_common_dir;
    let is_bare = git_dir_is_bare(&git_dir);

    Some(GitWorktreeInfo {
        repo_root,
        git_dir,
        git_common_dir,
        is_bare,
        is_linked_worktree,
    })
}

pub fn git_space_metadata(cwd: &Path) -> Option<GitSpaceMetadata> {
    let info = git_worktree_info(cwd)?;
    Some(git_space_metadata_from_info(&info))
}

pub(crate) fn automatic_workspace_label(cwd: &Path, repo_root: &Path) -> String {
    repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_label_from_cwd(cwd))
}

pub(super) fn git_space_metadata_from_info(info: &GitWorktreeInfo) -> GitSpaceMetadata {
    let key = canonicalize_best_effort_path(&info.git_common_dir)
        .display()
        .to_string();
    let common_dir_name = info
        .git_common_dir
        .file_name()
        .and_then(|name| name.to_str());
    let label_path = match common_dir_name {
        Some(".git") => info.git_common_dir.parent().unwrap_or(&info.repo_root),
        Some(".bare") => embedded_bare_repo_container(info).unwrap_or(&info.git_common_dir),
        _ => &info.git_common_dir,
    };
    let repo_name = label_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo")
        .to_string();
    GitSpaceMetadata {
        key,
        repo_name,
        repo_root: info.repo_root.clone(),
        is_linked_worktree: info.is_linked_worktree,
    }
}

fn embedded_bare_repo_container(info: &GitWorktreeInfo) -> Option<&Path> {
    let parent = info.git_common_dir.parent()?;
    let parent_git_dir = git_dir_for_repo_root(parent)?;
    (canonicalize_best_effort_path(&parent_git_dir) == info.git_common_dir).then_some(parent)
}

pub(super) fn canonicalize_best_effort_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn git_common_dir_for_git_dir(git_dir: &Path) -> PathBuf {
    let commondir = git_dir.join("commondir");
    let Ok(contents) = std::fs::read_to_string(commondir) else {
        return git_dir.to_path_buf();
    };
    let path = Path::new(contents.trim());
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        git_dir.join(path)
    }
}

pub(super) fn git_dir_for_repo_root(repo_root: &Path) -> Option<PathBuf> {
    let git_path = repo_root.join(".git");
    if git_path.is_dir() {
        return Some(git_path);
    }

    if let Ok(gitdir) = std::fs::read_to_string(&git_path) {
        if let Some(relative) = gitdir.trim().strip_prefix("gitdir:").map(str::trim) {
            let resolved = Path::new(relative);
            return Some(if resolved.is_absolute() {
                resolved.to_path_buf()
            } else {
                repo_root.join(resolved)
            });
        }
    }

    if path_is_git_dir_layout(repo_root) && git_dir_is_bare(repo_root) {
        return Some(repo_root.to_path_buf());
    }

    None
}

fn path_is_git_dir_layout(path: &Path) -> bool {
    path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir()
}

fn git_dir_is_bare(git_dir: &Path) -> bool {
    read_git_config_value(&git_dir.join("config"), "core", "bare")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn read_git_config_value(path: &Path, section: &str, key: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut in_section = false;
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section_name) = simple_git_config_section(line) {
            in_section = section_name.eq_ignore_ascii_case(section);
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(key) {
            return Some(strip_git_config_comment(value).trim().to_string());
        }
    }
    None
}

fn simple_git_config_section(line: &str) -> Option<&str> {
    let section = line.strip_prefix('[')?.split_once(']')?.0.trim();
    (!section.contains('"')).then_some(section)
}

fn strip_git_config_comment(value: &str) -> &str {
    let value = value.trim();
    for marker in ['#', ';'] {
        if let Some((prefix, _)) = value.split_once(marker) {
            if prefix.chars().next_back().is_some_and(char::is_whitespace) {
                return prefix;
            }
        }
    }
    value
}

pub(super) fn git_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };

    loop {
        if git_dir_for_repo_root(&current)
            .map(|git_dir| git_dir.join("HEAD").is_file())
            .unwrap_or(false)
        {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[cfg(test)]
#[path = "tests/git_support_test.rs"]
pub(super) mod test_support;

// Preserve the existing test namespace after merging the production forwarding layer.
#[cfg(test)]
mod discovery {
    use super::*;
    include!("tests/git_discovery_test.rs");
}
