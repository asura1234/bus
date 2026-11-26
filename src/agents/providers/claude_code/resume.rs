//! Reconstruct only the already-validated launch's Claude capture arguments.

use std::path::{Path, PathBuf};

use super::super::{
    hook_json::{resume_hooks, HookContext, HookContract},
    ProviderKind,
};
use super::statusline;

pub(crate) fn has_transcript(session: &str) -> bool {
    let default = std::env::home_dir().map(|home| home.join(".claude"));
    let configured = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let filename = format!("{session}.jsonl");
    configured.into_iter().chain(default).any(|config| {
        let projects = match std::fs::read_dir(config.join("projects")) {
            Ok(projects) => projects,
            Err(error) => return error.kind() != std::io::ErrorKind::NotFound,
        };
        projects.into_iter().any(|project| match project {
            Ok(project) if project.file_type().is_ok_and(|kind| kind.is_file()) => false,
            Ok(project) => match std::fs::metadata(project.path().join(&filename)) {
                Ok(metadata) => metadata.is_file() && metadata.len() > 0,
                Err(error) => error.kind() != std::io::ErrorKind::NotFound,
            },
            Err(_) => true,
        })
    })
}

pub(crate) fn capture_args(
    hooks: &HookContext,
    _project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    let path = hooks.spool.join("claude-settings.json");
    let rebound = resume_hooks(
        &path,
        HookContract::for_provider(ProviderKind::ClaudeCode),
        &hooks.binary,
    )?;
    let rebound =
        statusline::rebind(&hooks.spool, &hooks.binary).map_err(|e| e.to_string())? || rebound;
    let args = vec!["--settings".into(), path.to_string_lossy().into_owned()];
    Ok((path, args, rebound))
}
