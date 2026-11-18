//! AGY interactive launch preparation, option validation, and session adoption.
use super::{project_hooks, statusline};
use crate::agents::providers::{
    hook_json::{self, HookContext},
    launch::{self as common, LaunchSpec, PreparedLaunch, SetupNotice},
    ProviderKind,
};
use std::path::Path;

const DISALLOWED_SUBCOMMANDS: &[&str] = &[
    "acp",
    "agent",
    "agents",
    "changelog",
    "help",
    "install",
    "mcp",
    "mic-serve",
    "models",
    "plugin",
    "plugins",
    "remote-control",
    "update",
];

pub(crate) fn runtime_env() -> Vec<(String, String)> {
    vec![
        (
            "AGY_CLI_DISABLE_AUTO_UPDATE".into(),
            std::env::var("AGY_CLI_DISABLE_AUTO_UPDATE").unwrap_or_else(|_| "1".into()),
        ),
        (
            "AGY_CLI_OSC7501".into(),
            std::env::var("AGY_CLI_OSC7501").unwrap_or_else(|_| "on".into()),
        ),
    ]
}

pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["--conversation".into(), session.into()]
}

pub(crate) fn option_arity(name: &str) -> Option<usize> {
    match name {
        "--add-dir"
        | "--agent"
        | "--conversation"
        | "--effort"
        | "-i"
        | "--prompt-interactive"
        | "--input-format"
        | "--json-schema"
        | "--log-file"
        | "--mode"
        | "--model"
        | "--output-format"
        | "-p"
        | "--print"
        | "--print-timeout"
        | "--project"
        | "--prompt"
        | "--settings" => Some(1),
        "-c"
        | "--continue"
        | "--dangerously-skip-permissions"
        | "--disable-slash-commands"
        | "--new-project"
        | "--remote-agent"
        | "--remote-control"
        | "--sandbox"
        | "-h"
        | "--help"
        | "-v"
        | "--version" => Some(0),
        _ => None,
    }
}

pub(crate) fn setup_notice(root: &Path) -> SetupNotice {
    SetupNotice {
        path: project_hooks::hooks_path(root),
        message: common::hook_notice_message("agy-hook"),
    }
}

pub(crate) fn validate_args(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err("Launch arguments cannot contain NUL".into());
    }
    if args.iter().any(|arg| arg == "--") {
        return Err(
            "agy launch arguments cannot include a positional '--' tail; Bus owns interactive prompt delivery"
                .into(),
        );
    }
    for index in common::argument_positions(ProviderKind::Agy, args) {
        let arg = args[index].as_str();
        if !arg.starts_with('-') {
            if DISALLOWED_SUBCOMMANDS.contains(&arg) {
                return Err(format!(
                    "agy subcommand {arg:?} disables the interactive terminal Bus needs; see docs/launch-args.md"
                ));
            }
            return Err(format!(
                "agy positional argument {arg:?} replaces Bus's interactive prompt ownership; see docs/launch-args.md"
            ));
        }
        let name = arg.split_once('=').map_or(arg, |(name, _)| name);
        if matches!(
            name,
            "-p" | "--print"
                | "--prompt"
                | "-c"
                | "--continue"
                | "-i"
                | "--prompt-interactive"
                | "--remote-agent"
                | "-h"
                | "--help"
                | "-v"
                | "--version"
        ) {
            return Err(format!(
                "agy launch argument {name} disables the local interactive terminal or session ownership Bus needs; see docs/launch-args.md"
            ));
        }
        if matches!(name, "--settings" | "--conversation")
            && !arg.contains('=')
            && args
                .get(index + 1)
                .is_none_or(|val| val.is_empty() || val == "--")
        {
            return Err(format!("agy launch argument {name} requires a value"));
        }
    }
    Ok(())
}

/// Extract a single `--conversation <uuid>` or `--conversation=<uuid>` selector.
pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let selectors: Vec<usize> = common::argument_positions(ProviderKind::Agy, args)
        .into_iter()
        .filter(|&index| {
            args[index] == "--conversation" || args[index].starts_with("--conversation=")
        })
        .collect();
    if selectors.len() > 1 {
        return Err("Supply at most one --conversation UUID when adopting an AGY session".into());
    }
    let Some(&index) = selectors.first() else {
        return Ok(None);
    };
    let (value, count) = if let Some((_, value)) = args[index].split_once('=') {
        (value.to_owned(), 1)
    } else {
        let value = args
            .get(index + 1)
            .ok_or("agy launch argument --conversation requires a conversation UUID")?
            .clone();
        (value, 2)
    };
    if !common::is_session_uuid(&value) {
        return Err(
            "AGY session adoption requires an exact conversation UUID with --conversation".into(),
        );
    }
    args.drain(index..index + count);
    Ok(Some(value))
}

/// Strip one-time conversation selectors and user `--settings` (which `install`
/// already merged into `<spool>/agy-settings.json`, reused by `resume::capture_args`).
pub(crate) fn resumable_options(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') {
            index += 1;
            continue;
        }
        let name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        if matches!(
            name,
            "--conversation" | "-c" | "--continue" | "-i" | "--prompt-interactive" | "--settings"
        ) {
            index += 1;
            if !arg.contains('=') && option_arity(name) == Some(1) && index < args.len() {
                index += 1;
            }
            continue;
        }
        if arg.contains('=') {
            out.push(arg.clone());
            index += 1;
            continue;
        }
        match option_arity(name) {
            Some(0) => {
                out.push(arg.clone());
                index += 1;
            }
            Some(1) => {
                let end = (index + 2).min(args.len());
                out.extend_from_slice(&args[index..end]);
                index = end;
            }
            _ => {
                out.extend_from_slice(&args[index..]);
                break;
            }
        }
    }
    out
}

/// Remove `--settings` spans from saved options for an unstarted AGY launch,
/// because `resume::capture_args` supplies the already-merged `<spool>/agy-settings.json`.
pub(crate) fn unstarted_options(args: &[String]) -> Vec<String> {
    let mut out = args.to_vec();
    let _ = extract_user_settings(&mut out);
    out
}

/// Extract `--settings` values from `args`, removing those spans from effective argv
/// so Bus can pass a single merged `<spool>/agy-settings.json`.
fn extract_user_settings(args: &mut Vec<String>) -> (Vec<String>, bool) {
    let mut values = Vec::new();
    let mut explicit_sandbox = false;
    let mut kept = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--sandbox" || arg == "--sandbox=true" {
            explicit_sandbox = true;
            kept.push(arg.clone());
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--settings=") {
            values.push(value.to_owned());
            index += 1;
            continue;
        }
        if arg == "--settings" {
            if let Some(value) = args.get(index + 1) {
                values.push(value.clone());
                index += 2;
                continue;
            }
        }
        if !arg.contains('=') && arg.starts_with('-') {
            if let Some(1) = option_arity(arg.as_str()) {
                let end = (index + 2).min(args.len());
                kept.extend_from_slice(&args[index..end]);
                index = end;
                continue;
            }
        }
        kept.push(arg.clone());
        index += 1;
    }
    *args = kept;
    (values, explicit_sandbox)
}

pub(crate) fn validate_unstarted_argv(argv: &[String]) -> Result<(), String> {
    if argv
        .first()
        .and_then(|exe| Path::new(exe).file_name())
        .and_then(|name| name.to_str())
        != Some("agy")
    {
        return Err("Unstarted AGY launch has no matching executable".into());
    }
    let tail = &argv[1..];
    validate_args(tail)?;
    if common::argument_positions(ProviderKind::Agy, tail)
        .into_iter()
        .any(|idx| tail[idx] == "--conversation" || tail[idx].starts_with("--conversation="))
    {
        return Err("Adopted AGY session cannot be restored as an unstarted launch".into());
    }
    Ok(())
}

pub(crate) fn prepare(
    spec: &LaunchSpec,
    initialize: impl FnOnce(&HookContext) -> Result<(), String>,
) -> Result<PreparedLaunch, String> {
    let cwd = common::canonical_directory(&spec.cwd)?;
    let root = common::project_root(&cwd);
    let mut options = common::parse_path_tokens(&spec.extra_args).map_err(|e| e.to_string())?;
    let adopted_session = take_adopted_session(&mut options)?;
    validate_args(&options)?;
    if !common::binary_available(ProviderKind::Agy) {
        return Err("agy is not installed on PATH".into());
    }
    let notice = setup_notice(&root);
    if !project_hooks::hooks_are_current(&notice.path, &spec.hooks.binary) {
        if !spec.consent_project_hooks {
            return Err(format!(
                "Setup confirmation required for {}. {}",
                notice.path.display(),
                notice.message
            ));
        }
        project_hooks::install(&root, &spec.hooks.binary)?;
    }
    initialize(&spec.hooks)?;

    hook_json::atomic_write(
        &spec.hooks.spool.join("launch-options.json"),
        &serde_json::to_vec(&options).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if let Some(session) = &adopted_session {
        hook_json::atomic_write(
            &spec.hooks.spool.join("adopted-session"),
            session.as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    }

    let mut args = options;
    let (user_settings, explicit_sandbox) = extract_user_settings(&mut args);
    let settings = statusline::install(
        &spec.hooks.spool,
        &cwd,
        &spec.hooks.binary,
        &user_settings,
        explicit_sandbox,
    )?;
    if let Some(session) = &adopted_session {
        args.splice(0..0, resume_args(session));
    }
    common::apply_run_permissions(ProviderKind::Agy, &mut args);
    args.push("--settings".into());
    args.push(settings.to_string_lossy().into_owned());

    let mut env = spec.hooks.env();
    env.extend(runtime_env());
    Ok(PreparedLaunch {
        cwd,
        args,
        adopted_session,
        env,
    })
}
