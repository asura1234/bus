//! Claude's interactive argv and per-launch settings.

use super::hooks;
use crate::agents::providers::launch::{is_session_uuid, LaunchOption, LaunchSpec, PreparedLaunch};
use std::path::PathBuf;

pub(crate) fn supported_option(name: &str) -> Option<LaunchOption> {
    match name {
        "--model" | "--system-prompt" | "--append-system-prompt" => Some(LaunchOption::Value),
        "--verbose" | "--ax-screen-reader" => Some(LaunchOption::Flag),
        "--effort" => Some(LaunchOption::Choice(&[
            "low", "medium", "high", "xhigh", "max",
        ])),
        _ => None,
    }
}

/// Claude accepts one `--resume ID` or `--resume=ID` anywhere in user argv.
/// The common validator checks all remaining tokens after this extraction.
pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    let mut found = None;
    let mut index = 0;
    while index < args.len() {
        let attached = if args[index] == "--resume" {
            None
        } else if let Some(value) = args[index].strip_prefix("--resume=") {
            Some(value.to_owned())
        } else {
            index += 1;
            continue;
        };
        if found.is_some() {
            return Err("Give at most one session to resume".into());
        }
        args.remove(index);
        let value = match attached {
            Some(value) => value,
            None if index < args.len() => args.remove(index),
            None => String::new(),
        };
        if !is_session_uuid(&value) {
            return Err(format!(
                "Resume needs a session ID (a UUID), not {value:?}; pickers, --continue and --last are not supported"
            ));
        }
        found = Some(value);
    }
    Ok(found)
}

/// The common layer has already checked that the adopted session is a UUID.
pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["--resume".into(), session.into()]
}

pub(crate) fn runtime_args() -> Vec<String> {
    Vec::new()
}

/// Complete Claude preparation after common cwd/argv/availability validation
/// and caller-owned spool initialization/session reservation. No room or
/// terminal ownership data crosses this boundary.
pub(crate) fn prepare(
    spec: &LaunchSpec,
    cwd: PathBuf,
    mut args: Vec<String>,
    adopted_session: Option<String>,
) -> Result<PreparedLaunch, String> {
    let settings = hooks::install(&spec.hooks, &cwd)?;
    args.extend(["--settings".into(), settings.to_string_lossy().into_owned()]);
    Ok(PreparedLaunch {
        cwd,
        args,
        adopted_session,
        env: spec.hooks.env(),
    })
}

#[cfg(test)]
#[path = "tests/launch_test.rs"]
mod tests;
