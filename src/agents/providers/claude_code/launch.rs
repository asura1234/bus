//! Claude's interactive argv and per-launch settings.

use super::hooks;
use crate::agents::providers::{
    launch::{extend_generated_args, take_resume_option, LaunchSpec, PreparedLaunch},
    ProviderKind,
};
use std::path::PathBuf;

/// Claude accepts one `--resume ID` or `--resume=ID` anywhere in user argv.
/// The common validator checks all remaining tokens after this extraction.
pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    Ok(take_resume_option(ProviderKind::ClaudeCode, args))
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
    extend_generated_args(
        ProviderKind::ClaudeCode,
        &mut args,
        vec!["--settings".into(), settings.to_string_lossy().into_owned()],
    );
    let mut env = spec.hooks.env();
    env.extend(super::super::launch::runtime_env(ProviderKind::ClaudeCode));
    Ok(PreparedLaunch {
        cwd,
        args,
        adopted_session,
        env,
    })
}

#[cfg(test)]
#[path = "tests/launch_test.rs"]
mod tests;
