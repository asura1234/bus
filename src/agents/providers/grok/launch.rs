//! Managed Grok Build launches. PWD belongs to Bus; user argv is saved verbatim.
//! The canonical `grok` alias is validated, persisted, and prepended to PATH so
//! native launches and cold resumes execute the same selected installation.
use crate::agents::providers::{
    hook_json::{self, HookContext},
    launch::{self as common, LaunchSpec, PreparedLaunch, SetupNotice},
    ProviderKind,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
};
mod arguments;
mod identity;
pub(super) use arguments::runtime_additions;
pub(crate) use arguments::{option_arity, resumable_options, take_adopted_session, validate_args};

pub(crate) fn runtime_args() -> Vec<String> {
    vec![
        "--no-leader".into(),
        "--fullscreen".into(),
        "--no-auto-update".into(),
    ]
}
pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["--resume".into(), session.into()]
}

/// Only identity queries: never submit a prompt or run an inspection/model turn.
fn identity_output(path: &Path, arg: &str) -> Result<String, String> {
    let synthetic = driver_fixture(path)?;
    // Resolution may be called by a runtime thread. A separate scoped thread
    // keeps this synchronous API usable without nesting Tokio runtimes.
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                use tokio::io::AsyncReadExt;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                runtime.block_on(async {
                    let mut command = tokio::process::Command::new(path);
                    command
                        .arg(arg)
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .kill_on_drop(true);
                    for (key, _) in std::env::vars_os() {
                        if key.to_string_lossy().starts_with("BUS_")
                            || key.to_string_lossy().starts_with("HERDR_")
                        {
                            command.env_remove(key);
                        }
                    }
                    if synthetic {
                        command.env("BUS_TUI_FAKE_AGENT", "1");
                    }
                    #[cfg(unix)]
                    command.process_group(0);
                    let mut child = command
                        .spawn()
                        .map_err(|e| format!("Cannot query Grok Build identity: {e}"))?;
                    #[cfg(unix)]
                    let group = child.id();
                    let stdout = child
                        .stdout
                        .take()
                        .ok_or("Grok Build identity stdout is unavailable")?;
                    let mut bytes = Vec::new();
                    // Cold native startup and host contention can exceed two
                    // seconds; retain a finite per-query deadline and cleanup.
                    let finished =
                        tokio::time::timeout(std::time::Duration::from_secs(10), async {
                            let (_, status) = tokio::try_join!(
                                async {
                                    stdout.take(64 * 1024 + 1).read_to_end(&mut bytes).await?;
                                    if bytes.len() > 64 * 1024 {
                                        return Err(std::io::Error::other(
                                            "Grok Build identity output is too large",
                                        ));
                                    }
                                    Ok(())
                                },
                                child.wait()
                            )
                            .map_err(|e| e.to_string())?;
                            if !status.success() {
                                return Err("Grok Build identity query failed".into());
                            }
                            String::from_utf8(bytes)
                                .map_err(|_| "Invalid Grok Build identity output".into())
                        })
                        .await;
                    if !matches!(finished, Ok(Ok(_))) {
                        // A wrapper can leave a grandchild holding stdout open. Kill
                        // its private group as well as reaping the direct child.
                        #[cfg(unix)]
                        if let Some(group) = group {
                            // SAFETY: this is the fresh child's private process group.
                            unsafe {
                                libc::kill(-(group as i32), libc::SIGKILL);
                            }
                        }
                        let _ = child.start_kill();
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_millis(100),
                            child.wait(),
                        )
                        .await;
                    }
                    finished.map_err(|_| "Grok Build identity query timed out".to_owned())?
                })
            })
            .join()
            .map_err(|_| "Grok Build identity probe panicked".to_owned())?
    })
}

/// In the fake driver, reject every installation except this Bus executable's
/// alias. Preserve only the stand-in flag for its metadata query, never routing.
fn driver_fixture(path: &Path) -> Result<bool, String> {
    #[cfg(feature = "dev-tools")]
    if std::env::var("BUS_TUI_FAKE_AGENT").as_deref() == Ok("1")
        && std::env::var_os("BUS_TUI_OWNER").is_some()
    {
        let same = path
            .canonicalize()
            .ok()
            .zip(std::env::current_exe().ok())
            .is_some_and(|(alias, binary)| alias == binary);
        return if same {
            Ok(true)
        } else {
            Err("Isolated Grok acceptance requires this Bus's synthetic alias".into())
        };
    }
    let _ = path;
    Ok(false)
}

fn validate_executable(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.file_name().is_none_or(|name| name != "grok") || !path.is_file()
    {
        return Err("Grok Build requires an absolute executable alias named grok".into());
    }
    let version = identity_output(path, "--version")?;
    let version = version.trim();
    let build = version
        .strip_prefix("grok ")
        .and_then(|v| v.split_once(" ("));
    if !build.is_some_and(|(version, build)| {
        version.split('.').count() == 3
            && version
                .split('.')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            && build.ends_with(')')
            && build.len() > 1
    }) {
        return Err("Executable is not Grok Build (it may be the regex namesake)".into());
    }
    let help = identity_output(path, "--help")?;
    if !help.lines().any(|line| line.trim() == "Grok Build TUI")
        || !help.contains("Usage: grok")
        || ![
            "--resume",
            "--session-id",
            "--fullscreen",
            "--permission-mode",
            "--prompt-json",
        ]
        .iter()
        .all(|flag| help.contains(flag))
    {
        return Err("Executable does not identify as the Grok Build TUI".into());
    }
    Ok(())
}

fn resolve_identity() -> Result<identity::SavedExecutable, String> {
    let mut candidates = Vec::new();
    if let Some(home) = std::env::home_dir() {
        candidates.push(home.join(".grok/bin/grok"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let directory = if directory.is_absolute() {
                directory
            } else {
                std::env::current_dir()
                    .map_err(|e| e.to_string())?
                    .join(directory)
            };
            candidates.push(directory.join("grok"));
        }
    }
    let mut rejected = None;
    for path in candidates {
        match identity::validate(&path) {
            Ok(identity) => return Ok(identity),
            Err(reason) if path.is_file() => {
                tracing::warn!(event = "bus.grok.executable_rejected", reason = %reason,
                    "Installed Grok candidate failed identity validation");
                rejected.get_or_insert(reason);
            }
            Err(_) => {}
        }
    }
    Err(match rejected {
        Some(reason) => format!("Installed Grok Build candidate was rejected: {reason}"),
        None => {
            "Grok Build is not installed at ~/.grok/bin/grok or on PATH; regex grok is unsupported"
                .into()
        }
    })
}

const EXECUTABLE_FILE: &str = "grok-executable.json";

#[cfg(test)]
pub(crate) fn resolve_executable() -> Result<PathBuf, String> {
    resolve_identity().map(|identity| identity.alias)
}

pub(super) fn saved_executable(hooks: &HookContext) -> Result<PathBuf, String> {
    identity::load(hooks)
}

fn environment_for(
    hooks: &HookContext,
    executable: &Path,
) -> Result<HashMap<String, String>, String> {
    let directory = executable
        .parent()
        .ok_or("Grok Build executable has no directory")?;
    let mut paths = vec![directory.to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).filter(|path| path != directory));
    }
    let path = std::env::join_paths(paths)
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|_| "Grok launch PATH is not UTF-8")?;
    let mut env = hooks.env();
    env.insert("PATH".into(), path);
    Ok(env)
}

pub(crate) fn environment(hooks: &HookContext) -> Result<HashMap<String, String>, String> {
    let executable = saved_executable(hooks)?;
    environment_for(hooks, &executable)
}

pub(crate) fn setup_notice(root: &Path) -> SetupNotice {
    SetupNotice {
        path: root.join(".grok/hooks/bus.json"),
        message: common::hook_notice_message("grok-hook"),
    }
}

pub(crate) fn prepare(
    spec: &LaunchSpec,
    initialize: impl FnOnce(&HookContext) -> Result<(), String>,
) -> Result<PreparedLaunch, String> {
    let cwd = common::canonical_directory(&spec.cwd)?;
    let original = common::parse_path_tokens(&spec.extra_args).map_err(|e| e.to_string())?;
    validate_args(&original)?;
    let mut args = original.clone();
    let adopted_session = take_adopted_session(&mut args)?;
    let runtime = runtime_additions(&args);
    args.splice(0..0, runtime);
    if let Some(session) = &adopted_session {
        args.splice(0..0, resume_args(session));
    }
    let notice = setup_notice(&common::project_root(&cwd));
    if !spec.consent_project_hooks {
        return Err(format!(
            "Setup confirmation required for {}. {}",
            notice.path.display(),
            notice.message
        ));
    }
    let (_, _, selected) = super::project_hooks::update(
        &cwd,
        &spec.hooks.binary,
        super::project_hooks::Mode::Install,
        resolve_identity,
    )?;
    let env = environment_for(&spec.hooks, &selected.alias)?;
    selected.verify()?;
    initialize(&spec.hooks)?;
    for (name, bytes) in [
        (
            "launch-options.json",
            serde_json::to_vec(&original).map_err(|e| e.to_string())?,
        ),
        (
            EXECUTABLE_FILE,
            serde_json::to_vec(&selected).map_err(|e| e.to_string())?,
        ),
    ] {
        hook_json::atomic_write(&spec.hooks.spool.join(name), &bytes).map_err(|e| e.to_string())?;
    }
    if let Some(session) = &adopted_session {
        hook_json::atomic_write(
            &spec.hooks.spool.join("adopted-session"),
            session.as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    }
    common::apply_run_permissions(ProviderKind::Grok, &mut args);
    Ok(PreparedLaunch {
        cwd,
        args,
        adopted_session,
        env,
    })
}

#[cfg(test)]
#[path = "tests/arguments_test.rs"]
mod arguments_test;
#[cfg(test)]
#[path = "tests/launch_test.rs"]
pub(crate) mod launch_test;
#[cfg(test)]
#[path = "tests/settings_test.rs"]
mod settings_test;
