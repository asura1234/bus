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

/// The hook-consent notice is the same plain-language text for every
/// provider; only the callback command in its undo line differs.
pub(crate) fn hook_notice_message(callback: &str) -> String {
    format!(
        "Bus will add a few hooks to this project's agent settings, so room messages can reach this agent and its replies can reach the room. The agent may ask you to trust this project and the new hooks; accept them. Messages you send before the agent is ready wait and are delivered once it is. Your existing hooks stay as they are. To undo, delete the entries that run this Bus executable with --bus-callback {callback}."
    )
}

#[path = "launch/tokens.rs"]
mod tokens;
pub(crate) use tokens::{parse_path_tokens, PathParseError};
mod arguments;
pub(crate) use arguments::{argument_positions, resumable_options};

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

pub(crate) fn native_provider(label: &str) -> Option<ProviderKind> {
    match label {
        "codex" => Some(ProviderKind::Codex),
        "claude" => Some(ProviderKind::ClaudeCode),
        "cursor" => Some(ProviderKind::Cursor),
        _ => None,
    }
}

/// Updaters share an installation across panes. Respect an explicit Claude
/// environment setting; user/project settings can also override inherited env.
pub(crate) fn runtime_env(provider: ProviderKind) -> Vec<(String, String)> {
    match provider {
        ProviderKind::ClaudeCode => vec![(
            "DISABLE_AUTOUPDATER".into(),
            std::env::var("DISABLE_AUTOUPDATER").unwrap_or_else(|_| "1".into()),
        )],
        ProviderKind::Codex | ProviderKind::Cursor => Vec::new(),
    }
}

/// A run-scoped permission choice, kept separate from saved user options.
/// Inspect option positions so a literal prompt or option value is not
/// mistaken for an explicit bypass. Provider aliases suppress our copy too.
pub(crate) fn apply_run_permissions(provider: ProviderKind, args: &mut Vec<String>) {
    if !crate::utils::env::yolo_enabled() {
        return;
    }
    if provider == ProviderKind::Codex {
        arguments::remove_codex_approval_options(args);
    }
    let (flag, aliases): (&str, &[&str]) = match provider {
        ProviderKind::ClaudeCode => (
            "--dangerously-skip-permissions",
            &["--dangerously-skip-permissions"],
        ),
        ProviderKind::Codex => (
            "--yolo",
            &["--yolo", "--dangerously-bypass-approvals-and-sandbox"],
        ),
        ProviderKind::Cursor => ("--force", &["--force", "--yolo", "-f"]),
    };
    if !arguments::contains_flag(provider, args, aliases) {
        // Prefix rather than append: the provider must see it before any
        // literal tail or opaque option whose value arity Bus cannot know.
        args.insert(0, flag.into());
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
    extend_generated_args(provider, &mut args, runtime_args(provider));
    if let Some(session) = &adopted {
        args.splice(0..0, resume_args(provider, session));
    }
    Ok((args, adopted))
}

/// Preserve user argv and omit duplicate single-use runtime flags. Literal
/// tails and option values are not flags.
pub(crate) fn extend_generated_args(
    provider: ProviderKind,
    args: &mut Vec<String>,
    mut capture: Vec<String>,
) {
    if provider == ProviderKind::Codex && codex_update_check_disabled(args) {
        let mut index = 0;
        while index + 1 < capture.len() {
            if capture[index] == "-c" && capture[index + 1] == "check_for_update_on_startup=false" {
                capture.drain(index..index + 2);
            } else {
                index += 1;
            }
        }
    }
    let flag = match provider {
        ProviderKind::Codex => Some("--no-daemon"),
        ProviderKind::Cursor => Some("--disable-auto-update"),
        ProviderKind::ClaudeCode => None,
    };
    let supplied = flag.filter(|flag| {
        argument_positions(provider, args)
            .into_iter()
            .any(|index| args[index] == *flag)
    });
    let capture = capture
        .into_iter()
        .filter(|arg| Some(arg.as_str()) != supplied);
    // Prefix the complete user sequence when a token could be the literal
    // boundary. This also avoids splitting an option from a value named `--`.
    if args.iter().any(|arg| arg == "--") {
        args.splice(0..0, capture);
    } else {
        args.extend(capture);
    }
}

fn codex_update_check_disabled(args: &[String]) -> bool {
    // Codex config overrides are ordered. An earlier `false` must not mask a
    // later `true` when deciding whether our final safety override is present.
    argument_positions(ProviderKind::Codex, args)
        .into_iter()
        .filter_map(|index| match args[index].as_str() {
            "-c" | "--config" => args.get(index + 1).map(String::as_str),
            arg => arg.strip_prefix("--config="),
        })
        .filter_map(|value| value.split_once('='))
        .filter(|(key, _)| key.trim() == "check_for_update_on_startup")
        .map(|(_, value)| value.trim())
        .next_back()
        == Some("false")
}

pub(crate) fn validate_args(provider: ProviderKind, args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err("Launch arguments cannot contain NUL".into());
    }
    for index in argument_positions(provider, args) {
        let arg = args[index].as_str();
        let incompatible = match provider {
            ProviderKind::Codex => matches!(arg, "exec" | "e" | "review"),
            ProviderKind::ClaudeCode => matches!(
                arg,
                "-p" | "--print" | "--bg" | "--background" | "--bare" | "--safe-mode"
            ),
            ProviderKind::Cursor => matches!(arg, "-p" | "--print"),
        };
        if incompatible {
            return Err(format!("{} launch argument {arg} disables the interactive terminal or observation hooks Bus needs; see docs/launch-args.md", provider_kind(provider)));
        }
    }
    Ok(())
}

/// Only unambiguous UUID selectors are normalized for ownership reservation.
/// Native pickers, names and duplicate selectors remain the provider's input.
pub(crate) fn take_resume_option(provider: ProviderKind, args: &mut Vec<String>) -> Option<String> {
    let selectors: Vec<_> = argument_positions(provider, args)
        .into_iter()
        .filter(|&index| {
            args[index] == "--resume"
                || args[index].starts_with("--resume=")
                || (provider == ProviderKind::ClaudeCode && args[index] == "-r")
        })
        .collect();
    let [index] = selectors.as_slice() else {
        return None;
    };
    let index = *index;
    let (value, count) = if let Some((_, value)) = args[index].split_once('=') {
        (value.to_owned(), 1)
    } else {
        (args.get(index + 1)?.clone(), 2)
    };
    if !is_session_uuid(&value) {
        return None;
    }
    args.drain(index..index + count);
    Some(value)
}

pub(crate) fn reserved_session(spool: &Path) -> Option<String> {
    std::fs::read_to_string(spool.join(ADOPTED_SESSION_FILE))
        .ok()
        .map(|session| session.trim().to_owned())
        .filter(|session| !session.is_empty())
}

/// User options belong to the launch, independently of generated capture
/// arguments and the conversation it currently owns. Never invent missing
/// flags into this record or re-interpret user choices on resume. Run-scoped
/// permissions are applied separately after these options are loaded.
pub(crate) fn saved_options(provider: ProviderKind, spool: &Path) -> Result<Vec<String>, String> {
    let bytes = match std::fs::read(spool.join("launch-options.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Cannot read saved launch options: {error}")),
    };
    let options: Vec<String> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid saved launch options: {error}"))?;
    validate_args(provider, &options)?;
    Ok(options)
}

pub(crate) fn binary_available(provider: ProviderKind) -> bool {
    executable_available(executable(provider))
}

pub(crate) fn executable_available(binary: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(binary).is_file()))
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
    let (mut args, adopted_session) = launch_args(spec.provider, &spec.extra_args)?;
    if !binary_available(spec.provider) {
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
    let mut options = parse_path_tokens(&spec.extra_args).map_err(|error| error.to_string())?;
    take_adopted_session(spec.provider, &mut options)?;
    hook_json::atomic_write(
        &spec.hooks.spool.join("launch-options.json"),
        &serde_json::to_vec(&options).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if let Some(session) = &adopted_session {
        hook_json::atomic_write(
            &spec.hooks.spool.join(ADOPTED_SESSION_FILE),
            session.as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    }
    apply_run_permissions(spec.provider, &mut args);
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
