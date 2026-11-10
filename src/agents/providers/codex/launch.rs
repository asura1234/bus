//! Codex-specific interactive argv and project setup metadata.
use super::super::{
    launch::{argument_positions, is_session_uuid, SetupNotice},
    ProviderKind,
};
use std::path::Path;

/// Codex resolves --cd relative to its launch shell, before loading project hooks.
pub(crate) fn project_directory(cwd: &Path, args: &[String]) -> Result<std::path::PathBuf, String> {
    for index in argument_positions(ProviderKind::Codex, args) {
        let arg = &args[index];
        let value = if matches!(arg.as_str(), "--cd" | "-C") {
            Some(
                args.get(index + 1)
                    .ok_or("Codex --cd requires a directory")?
                    .as_str(),
            )
        } else {
            arg.strip_prefix("--cd=").or_else(|| {
                arg.strip_prefix("-C")
                    .map(|value| value.strip_prefix('=').unwrap_or(value))
            })
        };
        if let Some(value) = value {
            if value.is_empty() {
                return Err("Codex --cd requires a directory".into());
            }
            let path = cwd
                .join(value)
                .canonicalize()
                .map_err(|error| format!("Codex --cd {value:?} cannot be read: {error}"))?;
            if !path.is_dir() {
                return Err("Codex --cd must name an existing directory".into());
            }
            return Ok(path);
        }
    }
    Ok(cwd.to_path_buf())
}

/// A matching hook file is not usable when Codex explicitly distrusts its
/// project layer. Unknown projects still get Codex's normal trust review.
pub(crate) fn validate_project_trust(cwd: &Path) -> Result<(), String> {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".codex")))
        .ok_or("Codex configuration directory is unavailable")?;
    let config = home.join("config.toml");
    let text = match std::fs::read_to_string(&config) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(format!(
                "Cannot read Codex trust settings at {}; repair access before adding the agent",
                config.display()
            ))
        }
    };
    // Parser diagnostics can include source lines containing credentials.
    let document: toml::Table = toml::from_str(&text).map_err(|_| {
        format!(
            "Cannot parse Codex trust settings at {}; repair the file before adding the agent",
            config.display()
        )
    })?;
    let Some(projects) = document.get("projects").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    let root = super::super::launch::project_root(cwd);
    let root = root.canonicalize().unwrap_or(root);
    // Codex prefers the worktree's own trust level, then inherits the main
    // repository's. An entry with no trust level still uses that inheritance.
    let trust_root = if project_trust_level(projects, &root).is_some() {
        root.clone()
    } else {
        main_repository_root(&root).unwrap_or_else(|| root.clone())
    };
    if project_trust_level(projects, &trust_root) == Some("untrusted") {
        return Err(format!(
            "Codex project {} is marked untrusted in {}; project hooks at {} cannot load. If you trust this directory, set its project trust_level to \"trusted\" in that file, review /hooks in Codex, then re-add the agent; or choose a trusted --pwd.",
            trust_root.display(), config.display(), root.join(".codex/hooks.json").display()
        ));
    }
    Ok(())
}

fn project_trust_level<'a>(projects: &'a toml::Table, root: &Path) -> Option<&'a str> {
    projects
        .get(root.to_string_lossy().as_ref())
        .or_else(|| {
            projects.iter().find_map(|(path, project)| {
                Path::new(path)
                    .canonicalize()
                    .is_ok_and(|path| path == root)
                    .then_some(project)
            })
        })
        .and_then(|project| project.get("trust_level"))
        .and_then(toml::Value::as_str)
}

fn main_repository_root(root: &Path) -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let common = std::path::PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    common.parent().map(Path::to_path_buf)
}

/// Each launch hosts its own runtime so hooks inherit that launch's environment.
pub(crate) fn runtime_args() -> Vec<String> {
    vec![
        "--no-daemon".into(),
        "-c".into(),
        "check_for_update_on_startup=false".into(),
        // Draft transfers use these verified native actions. Codex rejects
        // colliding user bindings at startup before any composer can be edited.
        "-c".into(),
        "tui.keymap.global.open_external_editor=[\"ctrl-g\"]".into(),
        "-c".into(),
        "tui.keymap.editor.delete_backward=[\"backspace\"]".into(),
    ]
}

pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["resume".into(), session.into()]
}

/// Options can precede the command; option values named `resume` are literal.
pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let Some(index) = argument_positions(ProviderKind::Codex, args)
        .into_iter()
        .find(|&index| !args[index].starts_with('-'))
    else {
        return Ok(None);
    };
    if args[index] != "resume" {
        return Ok(None);
    }
    let Some(value) = args
        .get(index + 1)
        .filter(|value| is_session_uuid(value))
        .cloned()
    else {
        return Ok(None);
    };
    args.drain(index..index + 2);
    Ok(Some(value))
}

/// The caller resolves the project root before requesting the notice.
pub(crate) fn setup_notice(root: &Path) -> SetupNotice {
    SetupNotice {
        path: root.join(".codex/hooks.json"),
        message: super::super::launch::hook_notice_message("codex-hook"),
    }
}

/// Pin the TUI executable and opt this managed pane into native runtime ownership.
/// Non-Unix/legacy launches retain their ordinary private TUI runtime.
#[cfg(unix)]
pub(crate) fn environment(
    hooks: &super::super::hook_json::HookContext,
) -> std::collections::HashMap<String, String> {
    let mut env = hooks.env();
    if let Some(path) = crate::utils::env::path_var() {
        if let Some(directory) = std::env::split_paths(&path).find(|dir| {
            dir.is_absolute() && crate::utils::env::is_executable_file(&dir.join("codex"))
        }) {
            let paths = std::iter::once(directory).chain(std::env::split_paths(&path));
            if let Ok(path) = std::env::join_paths(paths) {
                env.insert("PATH".into(), path.to_string_lossy().into_owned());
                env.insert("BUS_CODEX_NATIVE".into(), "1".into());
            }
        }
    }
    env
}

#[cfg(not(unix))]
pub(crate) fn environment(
    hooks: &super::super::hook_json::HookContext,
) -> std::collections::HashMap<String, String> {
    hooks.env()
}
