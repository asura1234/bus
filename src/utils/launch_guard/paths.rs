//! Path validation shared by test launch guards and pre-spawn fixture setup.
use std::{
    io,
    path::{Component, Path, PathBuf},
};

pub(crate) const PROTECTED_HOME: &str = "TEST_BUS_PROTECTED_HOME";

/// Resolve links and missing leaves component by component, checking links before
/// following them as well as their targets. A later `..` cannot hide a protected visit.
fn resolved(path: &Path, protected: &[PathBuf], links: usize) -> io::Result<PathBuf> {
    if !path.is_absolute() || links > 40 {
        return Err(io::Error::other(
            "test isolation refused: invalid test path",
        ));
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
        refuse_protected(&result, protected)?;
        match std::fs::symlink_metadata(&result) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target = std::fs::read_link(&result)?;
                let target = if target.is_absolute() {
                    target
                } else {
                    result.parent().unwrap_or(Path::new("/")).join(target)
                };
                result = resolved(&target, protected, links + 1)?;
            }
            Ok(_) => result = result.canonicalize()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        refuse_protected(&result, protected)?;
    }
    Ok(result)
}

fn refuse_protected(path: &Path, protected: &[PathBuf]) -> io::Result<()> {
    if protected.iter().any(|root| path.starts_with(root)) {
        return Err(io::Error::other(
            "test isolation refused: path enters protected HOME state",
        ));
    }
    Ok(())
}

pub(crate) fn validate(home: &Path, data: Option<&Path>, protected: &Path) -> io::Result<()> {
    let protected = resolved(protected, &[], 0)?;
    if resolved(home, &[], 0)? == protected {
        return Err(io::Error::other(
            "test isolation refused: set a scratch HOME before booting Bus",
        ));
    }
    let mut roots = Vec::new();
    for directory in [
        ".local/share/bus",
        ".claude",
        ".agents",
        ".codex",
        ".cursor",
        ".grok",
        ".kimi-code",
    ] {
        let root = protected.join(directory);
        roots.push(resolved(&root, &[], 0)?);
        roots.push(root);
    }
    for directory in [
        ".local/share/bus",
        ".claude/skills",
        ".agents/skills",
        ".codex",
        ".cursor/skills",
        ".grok/skills",
        ".kimi-code/skills",
    ] {
        resolved(&home.join(directory), &roots, 0)?;
    }
    if let Some(root) = data.filter(|path| path.is_absolute()) {
        resolved(root, &roots, 0)?;
    }
    Ok(())
}

/// A PTY chooses HOME as its initial cwd before the child can run its guard.
#[cfg(test)]
pub(crate) fn prepare_fixture_home(home: &Path) -> io::Result<()> {
    let protected = std::env::var_os(PROTECTED_HOME)
        .map(PathBuf::from)
        .or_else(std::env::home_dir)
        .ok_or_else(|| io::Error::other("test isolation refused: protected HOME is unavailable"))?;
    validate(home, None, &protected)?;
    std::fs::create_dir_all(home)
}

#[cfg(test)]
#[path = "../tests/launch_paths_test.rs"]
mod tests;
