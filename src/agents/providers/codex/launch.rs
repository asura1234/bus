//! Codex-specific interactive argv and project setup metadata.
use super::super::{
    launch::{argument_positions, is_session_uuid, SetupNotice},
    ProviderKind,
};
use std::path::Path;

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
    let root = super::super::launch::project_root(cwd);
    let root = root.canonicalize().unwrap_or(root);
    let untrusted = document
        .get("projects")
        .and_then(toml::Value::as_table)
        .is_some_and(|projects| {
            projects.iter().any(|(path, project)| {
                let path = Path::new(path);
                project.get("trust_level").and_then(toml::Value::as_str) == Some("untrusted")
                    && (path == root || path.canonicalize().is_ok_and(|path| path == root))
            })
        });
    if untrusted {
        return Err(format!(
            "Codex project {} is marked untrusted in {}; project hooks at {} cannot load. If you trust this directory, set its project trust_level to \"trusted\" in that file, review /hooks in Codex, then re-add the agent; or choose a trusted --pwd.",
            root.display(), config.display(), root.join(".codex/hooks.json").display()
        ));
    }
    Ok(())
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
