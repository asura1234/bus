//! Room-free inputs and outputs shared by provider launch adapters.

use super::{
    claude_code, codex, cursor,
    hook_json::{self, HookContext, HookContract},
    ProviderKind,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Provider preparation input. The coordinator retains AddAgent, agent-name
/// validation, room policy, session ownership and callback-manifest creation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchSpec {
    pub(crate) provider: ProviderKind,
    /// Original absolute or ~/ input; common preparation canonicalizes it.
    pub(crate) cwd: String,
    /// Original user input, parsed and validated below the messaging boundary.
    pub(crate) extra_args: String,
    /// Explicit permission to install project observation hooks. Claude's
    /// per-launch settings retain their existing separate behavior.
    pub(crate) consent_project_hooks: bool,
    /// Capture paths and launch identity supplied by the caller, with no
    /// knowledge of the room or agent to which the callbacks will belong.
    pub(crate) hooks: HookContext,
}

/// Provider preparation result. This contains argv, never a shell command,
/// and neither starts a terminal nor binds a session to a room participant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedLaunch {
    pub(crate) cwd: PathBuf,
    pub(crate) args: Vec<String>,
    /// Existing provider session adopted by this launch, if any. Whether that
    /// session is already owned or may be bound remains a coordinator decision.
    pub(crate) adopted_session: Option<String>,
    pub(crate) env: HashMap<String, String>,
}

/// Provider setup information presented by the caller before consent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SetupNotice {
    pub(crate) path: PathBuf,
    pub(crate) message: String,
}

#[path = "launch/tokens.rs"]
mod tokens;
pub(crate) use tokens::{parse_path_tokens, PathParseError};

pub(crate) enum LaunchOption {
    Flag,
    Value,
    Choice(&'static [&'static str]),
}

pub(crate) const fn provider_kind(provider: ProviderKind) -> &'static str {
    provider.label()
}

pub(crate) const fn executable(provider: ProviderKind) -> &'static str {
    provider.executable()
}

pub(crate) fn runtime_args(provider: ProviderKind) -> Vec<String> {
    match provider {
        ProviderKind::Codex => codex::launch::runtime_args(),
        ProviderKind::ClaudeCode => claude_code::launch::runtime_args(),
        ProviderKind::Cursor => cursor::launch::runtime_args(),
    }
}

pub(crate) fn take_adopted_session(
    provider: ProviderKind,
    args: &mut Vec<String>,
) -> Result<Option<String>, String> {
    match provider {
        ProviderKind::Codex => codex::launch::take_adopted_session(args),
        ProviderKind::ClaudeCode => claude_code::launch::take_adopted_session(args),
        ProviderKind::Cursor => cursor::launch::take_adopted_session(args),
    }
}

fn resume_args(provider: ProviderKind, session: &str) -> Vec<String> {
    match provider {
        ProviderKind::Codex => codex::launch::resume_args(session),
        ProviderKind::ClaudeCode => claude_code::launch::resume_args(session),
        ProviderKind::Cursor => cursor::launch::resume_args(session),
    }
}

fn supported_option(provider: ProviderKind, name: &str) -> Option<LaunchOption> {
    match provider {
        ProviderKind::Codex => codex::launch::supported_option(name),
        ProviderKind::ClaudeCode => claude_code::launch::supported_option(name),
        ProviderKind::Cursor => cursor::launch::supported_option(name),
    }
}

pub(crate) fn canonical_directory(input: &str) -> Result<PathBuf, String> {
    let path = if input == "~" || input.starts_with("~/") {
        let home = std::env::home_dir().ok_or("Home directory unavailable")?;
        if input == "~" {
            home
        } else {
            home.join(&input[2..])
        }
    } else {
        PathBuf::from(input)
    };
    if !path.is_absolute() {
        return Err("Agent PWD must be an absolute path or ~/ path".into());
    }
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.is_dir() {
        return Err("Agent PWD must be an existing directory".into());
    }
    Ok(path)
}

pub(crate) fn adopted_session(
    provider: ProviderKind,
    extra_args: &str,
) -> Result<Option<String>, String> {
    let mut args = parse_path_tokens(extra_args).map_err(|e| e.to_string())?;
    take_adopted_session(provider, &mut args)
}

pub(crate) fn is_session_uuid(value: &str) -> bool {
    value.len() == 36
        && value.char_indices().all(|(index, c)| match index {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

pub(crate) fn launch_args(
    provider: ProviderKind,
    extra_args: &str,
) -> Result<(Vec<String>, Option<String>), String> {
    let mut args = parse_path_tokens(extra_args).map_err(|e| e.to_string())?;
    let adopted = take_adopted_session(provider, &mut args)?;
    validate_args(provider, &args)?;
    if let Some(session) = &adopted {
        args.splice(0..0, resume_args(provider, session));
    }
    args.extend(runtime_args(provider));
    Ok((args, adopted))
}

pub(crate) fn validate_args(provider: ProviderKind, args: &[String]) -> Result<(), String> {
    let mut tokens = args.iter();
    while let Some(arg) = tokens.next() {
        if arg.contains('\0') {
            return Err("Launch arguments cannot contain NUL".into());
        }
        // Only Codex advertises -m. Support its separated and attached value
        // forms without accepting other short options or option clusters.
        let (name, attached) = if provider == ProviderKind::Codex && arg.starts_with("-m") {
            let rest = &arg[2..];
            (
                "-m",
                (!rest.is_empty()).then(|| rest.strip_prefix('=').unwrap_or(rest)),
            )
        } else {
            arg.split_once('=')
                .map_or((arg.as_str(), None), |(name, value)| (name, Some(value)))
        };
        let kind = supported_option(provider, name).ok_or_else(|| {
            format!("Unsupported {} launch argument {name}. Bus accepts only supported interactive model/display options, not subcommands or permission, hook, configuration, or workspace overrides.", provider_kind(provider))
        })?;
        if matches!(kind, LaunchOption::Flag) {
            if attached.is_some() {
                return Err(format!("Launch option {name} does not take a value"));
            }
            continue;
        }
        let value = attached
            .or_else(|| tokens.next().map(String::as_str))
            .filter(|value| !value.is_empty() && !value.starts_with('-') && !value.contains('\0'))
            .ok_or_else(|| {
                format!("Launch option {name} requires a nonempty value, not another option")
            })?;
        if let LaunchOption::Choice(choices) = kind {
            if !choices.contains(&value) {
                return Err(format!(
                    "Launch option {name} must be one of: {}",
                    choices.join(", ")
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn reserved_session(spool: &Path) -> Option<String> {
    std::fs::read_to_string(spool.join(ADOPTED_SESSION_FILE))
        .ok()
        .map(|session| session.trim().to_owned())
        .filter(|session| !session.is_empty())
}

pub(crate) fn project_root(cwd: &Path) -> PathBuf {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        }
        _ => cwd.to_path_buf(),
    }
}

const ADOPTED_SESSION_FILE: &str = "adopted-session";

pub(crate) fn setup_notice(provider: ProviderKind, cwd: &Path) -> Option<SetupNotice> {
    let root = project_root(cwd);
    match provider {
        ProviderKind::Codex => Some(codex::launch::setup_notice(&root)),
        ProviderKind::ClaudeCode => None,
        ProviderKind::Cursor => Some(cursor::launch::setup_notice(&root)),
    }
}

/// The caller initializes its routing manifest after project hook setup and
/// before session reservation or provider-specific per-launch settings.
pub(crate) fn prepare(
    spec: &LaunchSpec,
    initialize: impl FnOnce(&HookContext) -> Result<(), String>,
) -> Result<PreparedLaunch, String> {
    let cwd = canonical_directory(&spec.cwd)?;
    let (args, adopted_session) = launch_args(spec.provider, &spec.extra_args)?;
    let available = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.join(executable(spec.provider)).is_file())
    });
    if !available {
        return Err(format!(
            "{} is not installed on PATH",
            executable(spec.provider)
        ));
    }
    if let Some(notice) = setup_notice(spec.provider, &cwd) {
        if !spec.consent_project_hooks {
            return Err(format!(
                "Setup confirmation required for {}. {}",
                notice.path.display(),
                notice.message
            ));
        }
        hook_json::install_hooks(
            &notice.path,
            HookContract::for_provider(spec.provider),
            &spec.hooks.binary,
        )?;
    }
    initialize(&spec.hooks)?;
    if let Some(session) = &adopted_session {
        hook_json::atomic_write(
            &spec.hooks.spool.join(ADOPTED_SESSION_FILE),
            session.as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    }
    match spec.provider {
        ProviderKind::ClaudeCode => claude_code::launch::prepare(spec, cwd, args, adopted_session),
        ProviderKind::Codex | ProviderKind::Cursor => Ok(PreparedLaunch {
            cwd,
            args,
            adopted_session,
            env: spec.hooks.env(),
        }),
    }
}
