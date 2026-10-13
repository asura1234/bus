//! Per-launch source profiles. Claude filesystem arrays are additive, so its
//! profile deliberately keeps regular approval checks and grants no build rules.
use super::{LaunchSpec, PreparedLaunch};
use crate::agents::providers::{hook_json, ProviderKind};
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub(crate) const CLAUDE_SERVICE: &str = "claude-service-cli-v1";
pub(crate) const ORCHESTRATOR_NATIVE_APPROVAL: &str = "orchestrator-native-approval-v1";
pub(crate) const CODEX_READ_ONLY: &str = "codex-read-only-protected-overlap-v1";
pub(crate) const CODEX_WORKSPACE_WRITE: &str = "codex-workspace-write-v1";
pub(crate) const CLAUDE_WORKER: &str = "claude-additive-sandbox-regular-approval-v1";
pub(crate) const CURSOR_NATIVE_APPROVAL: &str = "cursor-native-approval-v1";

pub(crate) fn validate_extra_dirs(cwd: &Path, args: &mut [String]) -> Result<(), String> {
    let mut index = 0;
    while index < args.len() {
        let separated = args[index] == "--add-dir";
        let value = if separated {
            args.get(index + 1).map(String::as_str)
        } else {
            args[index].strip_prefix("--add-dir=")
        };
        if let Some(value) = value {
            let candidate = PathBuf::from(value);
            let candidate = if candidate.is_absolute() {
                candidate
            } else {
                cwd.join(candidate)
            };
            let root = candidate
                .canonicalize()
                .map_err(|e| format!("Extra directory unavailable: {e}"))?;
            if !root.is_dir() || !root.starts_with(cwd) {
                return Err("Extra writable directory must be a canonical descendant of the frozen worker root".into());
            }
            let canonical = root.to_string_lossy().into_owned();
            if separated {
                args[index + 1] = canonical;
                index += 1;
            } else {
                args[index] = format!("--add-dir={canonical}");
            }
        }
        index += 1;
    }
    Ok(())
}

pub(crate) fn protected_paths(hooks: &hook_json::HookContext) -> Vec<PathBuf> {
    let mut paths = vec![hooks.binary.clone()];
    if let Some(target) = hooks
        .binary
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "target"))
    {
        paths.push(target.to_owned());
    }
    if let Some(callbacks) = hooks
        .spool
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "callbacks"))
    {
        if let Some(data) = callbacks.parent() {
            paths.push(data.to_owned());
        }
    }
    paths
        .into_iter()
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect()
}

pub(crate) fn codex_read_only(
    hooks: &hook_json::HookContext,
    cwd: Option<&Path>,
    previously_read_only: bool,
) -> bool {
    previously_read_only
        || cwd.is_none_or(|cwd| {
            protected_paths(hooks)
                .iter()
                .any(|p| p.starts_with(cwd) || cwd.starts_with(p))
        })
}

fn private_scratch(hooks: &hook_json::HookContext) -> Result<PathBuf, String> {
    let name = crate::utils::time::digest(
        format!("{}:{}", hooks.launch_id, hooks.spool.display()).as_bytes(),
    );
    let path = std::env::temp_dir()
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(format!("bus-worker-{name}"));
    // A new launch never adopts a pre-existing path, including symlinks.
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&path)
        .map_err(|e| format!("Private worker scratch creation failed: {e}"))?;
    Ok(path)
}

pub(crate) fn codex_args(scratch: Option<&Path>, read_only: bool) -> Vec<String> {
    let mut roots: Vec<PathBuf> = scratch.into_iter().map(Path::to_path_buf).collect();
    if !read_only {
        if let Some(home) = std::env::home_dir() {
            for relative in [".cargo", ".rustup", "Library/Caches/zig", ".cache/zig"] {
                if let Ok(root) = home.join(relative).canonicalize() {
                    if root.is_dir() {
                        roots.push(root);
                    }
                }
            }
        }
    }
    let mut args = vec![
        "--sandbox".into(),
        if read_only {
            "read-only"
        } else {
            "workspace-write"
        }
        .into(),
        "--ask-for-approval".into(),
        "on-request".into(),
    ];
    for value in [
        format!("sandbox_workspace_write.writable_roots={}", json!(roots)),
        "sandbox_workspace_write.exclude_slash_tmp=true".into(),
        "sandbox_workspace_write.exclude_tmpdir_env_var=true".into(),
        "sandbox_workspace_write.network_access=false".into(),
    ] {
        args.extend(["-c".into(), value]);
    }
    args
}

pub(crate) fn cli_prefix(binary: &Path) -> String {
    crate::platform::remote_reattach_program(&binary.to_string_lossy())
}

pub(crate) fn service_env(binary: &Path, env: &mut HashMap<String, String>) -> Result<(), String> {
    let parent = binary.parent().ok_or("Bus executable has no directory")?;
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(parent.to_owned()).chain(std::env::split_paths(&inherited));
    let path = std::env::join_paths(paths).map_err(|e| e.to_string())?;
    env.insert("PATH".into(), path.to_string_lossy().into_owned());
    Ok(())
}

pub(crate) fn service_settings(hooks: &hook_json::HookContext) -> Result<(), String> {
    let path = hooks.spool.join("claude-settings.json");
    let mut settings = hook_json::read_resume_json(&path)?;
    let prefix = cli_prefix(&hooks.binary);
    let mut allow: Vec<_> = [
        "state",
        "agent dialog",
        "agent choose",
        "agent answer",
        "agent permission-decision",
    ]
    .into_iter()
    .map(|family| format!("Bash({prefix} {family} *)"))
    .collect();
    allow.push(format!("Bash({prefix} state)"));
    allow.extend(
        [
            "Read",
            "Glob",
            "Grep",
            "Bash(cat *)",
            "Bash(ls *)",
            "Bash(realpath *)",
            "Bash(grep *)",
            "Bash(head *)",
            "Bash(test *)",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    // This file is Bus-owned per launch; replace its service allowlist on rebind
    // so a stale executable never remains implicitly authorized.
    // Unlisted service work fails closed instead of making the service itself
    // wait for a permission handler. This does not bypass approval checks.
    settings["permissions"]["defaultMode"] = json!("dontAsk");
    settings["permissions"]["allow"] = json!(allow);
    #[cfg(unix)]
    {
        let data = hooks
            .spool
            .parent()
            .filter(|p| p.file_name().is_some_and(|n| n == "callbacks"))
            .and_then(Path::parent)
            .ok_or("Service callback path has no assigned session root")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        // Preserve inherited/managed sandbox settings. Only this assigned
        // protocol socket is added; no all-sockets or filesystem exception.
        settings["sandbox"]["network"]["allowUnixSockets"] = json!([data.join("control.sock")]);
    }
    hook_json::atomic_write(
        &path,
        &serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub(crate) fn worker_claude_settings(
    hooks: &hook_json::HookContext,
    cwd: &Path,
    scratch: &Path,
) -> Result<(), String> {
    let path = hooks.spool.join("claude-settings.json");
    let mut settings = hook_json::read_resume_json(&path)?;
    settings["permissions"]["defaultMode"] = json!("default");
    let protected = protected_paths(hooks);
    settings["sandbox"] = json!({
        "enabled": true,
        "autoAllowBashIfSandboxed": false,
        "filesystem": { "allowWrite": [cwd, scratch], "denyWrite": protected,
            "denyRead": ["~/.ssh", "~/.aws", "~/.config/gcloud", "~/.codex/auth.json", "~/.claude/.credentials.json"] },
        "network": { "allowedDomains": [] }
    });
    // Built-in file tools run outside the Bash sandbox; protect live Bus writes
    // there too, without inventing general read confinement.
    let deny: Vec<_> = protected
        .iter()
        .flat_map(|p| {
            [
                format!("Edit(/{})", p.display()),
                format!("Edit(/{}/**)", p.display()),
            ]
        })
        .collect();
    settings["permissions"]["deny"] = json!(deny);
    hook_json::atomic_write(
        &path,
        &serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub(crate) fn install(spec: &LaunchSpec, launch: &mut PreparedLaunch) -> Result<(), String> {
    if spec.permission_handler {
        if spec.provider != ProviderKind::ClaudeCode {
            return Err("Permission service requires Claude".into());
        }
        service_settings(&spec.hooks)?;
        service_env(&spec.hooks.binary, &mut launch.env)?;
        launch.args.extend([
            "--permission-mode".into(),
            "dontAsk".into(),
            "--tools".into(),
            "Read,Glob,Grep,Bash".into(),
            "--setting-sources".into(),
            String::new(),
        ]);
        launch.profile = CLAUDE_SERVICE.into();
        return Ok(());
    }
    if spec.orchestrator {
        launch.profile = ORCHESTRATOR_NATIVE_APPROVAL.into();
        return Ok(());
    }
    let scratch = private_scratch(&spec.hooks)?;
    launch
        .env
        .insert("TMPDIR".into(), scratch.to_string_lossy().into_owned());
    launch.profile = match spec.provider {
        ProviderKind::Codex => {
            let overlap = codex_read_only(&spec.hooks, Some(&launch.cwd), false);
            launch.args.extend(codex_args(Some(&scratch), overlap));
            if overlap {
                CODEX_READ_ONLY
            } else {
                CODEX_WORKSPACE_WRITE
            }
        }
        ProviderKind::ClaudeCode => {
            worker_claude_settings(&spec.hooks, &launch.cwd, &scratch)?;
            CLAUDE_WORKER
        }
        ProviderKind::Cursor => CURSOR_NATIVE_APPROVAL,
    }
    .into();
    launch.scratch_dir = Some(scratch);
    Ok(())
}

/// Remove only Bus-owned defaults when validating a saved unstarted argv. The
/// authoritative saved launch scope is revalidated before reconstructing them.
pub(crate) fn strip_codex_args(args: &mut Vec<String>) {
    let mut i = 0;
    while i + 1 < args.len() {
        let owned = match args[i].as_str() {
            "--sandbox" => ["workspace-write", "read-only"].contains(&args[i + 1].as_str()),
            "--ask-for-approval" => args[i + 1] == "on-request",
            "-c" => {
                args[i + 1].starts_with("sandbox_workspace_write.writable_roots=")
                    || [
                        "sandbox_workspace_write.exclude_slash_tmp=true",
                        "sandbox_workspace_write.exclude_tmpdir_env_var=true",
                        "sandbox_workspace_write.network_access=false",
                    ]
                    .contains(&args[i + 1].as_str())
            }
            _ => false,
        };
        if owned {
            args.drain(i..i + 2);
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod build_scope_tests {
    use super::*;
    #[test]
    fn codex_build_cache_roots_are_explicit_and_keep_approval() {
        let args = codex_args(Some(Path::new("/private/worker-scratch")), false);
        let roots = args
            .iter()
            .find(|a| a.starts_with("sandbox_workspace_write.writable_roots="))
            .unwrap();
        let home = std::env::home_dir().unwrap();
        for relative in [".cargo", ".rustup", "Library/Caches/zig", ".cache/zig"] {
            let path = home.join(relative);
            if path.is_dir() {
                assert!(
                    roots.contains(&*path.canonicalize().unwrap().to_string_lossy()),
                    "missing {}: {roots}",
                    path.display()
                );
            }
        }
        assert!(args
            .windows(2)
            .any(|a| a == ["--ask-for-approval", "on-request"]));
    }
}
