//! Per-agent Kimi home: no project or global hook configuration is changed.
use crate::agents::providers::{
    hook_json::{self, HookContext},
    launch::{self as common, LaunchSpec, PreparedLaunch},
    ProviderKind,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

const HOME_FILE: &str = "kimi-home.json";
const EXECUTABLE_FILE: &str = "kimi-executable.json";

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
struct Stamp {
    canonical: PathBuf,
    size: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
struct HomeRecord {
    path: PathBuf,
    source: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn home_record(path: &Path, source: &Path) -> Result<HomeRecord, String> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("Saved Kimi home is not a real directory".into());
    }
    #[cfg(unix)]
    {
        if meta.mode() & 0o077 != 0 {
            return Err("Saved Kimi home is not owner-only".into());
        }
    }
    Ok(HomeRecord {
        path: path.to_path_buf(),
        source: source.to_path_buf(),
        #[cfg(unix)]
        device: meta.dev(),
        #[cfg(unix)]
        inode: meta.ino(),
    })
}

fn stamp(path: &Path) -> Result<Stamp, String> {
    if !path.is_absolute() || path.file_name().is_none_or(|name| name != "kimi") {
        return Err("Kimi executable must be an absolute alias named kimi".into());
    }
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    let meta = canonical.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Kimi executable is not a file".into());
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    #[cfg(unix)]
    if meta.mode() & 0o111 == 0 {
        return Err("Kimi executable is not executable".into());
    }
    Ok(Stamp {
        canonical,
        size: meta.len(),
        modified: meta.modified().map_err(|e| e.to_string())?,
        #[cfg(unix)]
        device: meta.dev(),
        #[cfg(unix)]
        inode: meta.ino(),
        #[cfg(unix)]
        changed: (meta.ctime(), meta.ctime_nsec()),
    })
}

fn source_home() -> Result<PathBuf, String> {
    let path = match std::env::var_os("KIMI_CODE_HOME") {
        Some(home) => PathBuf::from(home),
        None => std::env::home_dir()
            .ok_or("Home directory unavailable")?
            .join(".kimi-code"),
    };
    if !path.is_absolute() {
        return Err("KIMI_CODE_HOME must be absolute".into());
    }
    path.canonicalize()
        .map_err(|e| format!("Kimi home is unavailable: {e}"))
}

pub(crate) fn source_binary_available() -> bool {
    source_home().is_ok_and(|home| home.join("bin/kimi").is_file())
        || std::env::home_dir().is_some_and(|home| home.join(".kimi-code/bin/kimi").is_file())
}

fn resolve_executable(source: &Path) -> Result<PathBuf, String> {
    let mut candidates = vec![source.join("bin/kimi")];
    if let Some(home) = std::env::home_dir() {
        candidates.push(home.join(".kimi-code/bin/kimi"));
    }
    if let Some(path) = crate::utils::env::path_var() {
        candidates.extend(
            std::env::split_paths(&path)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("kimi")),
        );
    }
    let path = candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or("Kimi Code is not installed")?;
    stamp(&path)?;
    let version = std::process::Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|e| e.to_string())?;
    let help = std::process::Command::new(&path)
        .arg("--help")
        .output()
        .map_err(|e| e.to_string())?;
    if !version.status.success()
        || !help.status.success()
        || !String::from_utf8_lossy(&version.stdout)
            .trim()
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        || !["--session", "--agent-file", "--yolo"]
            .iter()
            .all(|flag| String::from_utf8_lossy(&help.stdout).contains(flag))
    {
        return Err("Executable does not identify as Kimi Code".into());
    }
    Ok(path)
}

fn copy_private_file(source: &Path, target: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 2 * 1024 * 1024 {
        return Err("Kimi private-home source is not a regular file".into());
    }
    let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
    hook_json::atomic_write(target, &bytes).map_err(|e| e.to_string())
}

fn bus_managed_source(source: &Path) -> bool {
    source.file_name().is_some_and(|name| name == "kimi-home")
        && source
            .parent()
            .is_some_and(|parent| parent.join(HOME_FILE).exists())
}

// Kimi replaces individual token files with atomic renames, and its OAuth
// refresh lock is rooted separately in oauth/. Share both whole directories so
// every process sees the same token and participates in the same lock.
#[cfg(unix)]
fn share_auth(source: &Path, home: &Path) -> Result<(), String> {
    use std::os::unix::fs::{symlink, DirBuilderExt};
    if bus_managed_source(source) {
        return Err("A Bus-managed Kimi home cannot be an authentication source".into());
    }
    for folder in ["credentials", "oauth"] {
        let target = source.join(folder);
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700).recursive(true);
        builder.create(&target).map_err(|e| e.to_string())?;
        if !target.is_dir() {
            return Err("Kimi shared authentication directory is unavailable".into());
        }
        // Never overwrite a previous private copy or an unexpected link.
        symlink(&target, home.join(folder)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn share_auth(_source: &Path, _home: &Path) -> Result<(), String> {
    // Kimi 2.4.0 disables cross-process refresh locking on Windows.
    Err("Kimi shared authentication requires cross-process refresh locking".into())
}

fn verify_shared_auth(home: &Path, source: &Path) -> Result<(), String> {
    if !source.is_absolute() || bus_managed_source(source) {
        return Err("Saved Kimi authentication source is invalid".into());
    }
    for folder in ["credentials", "oauth"] {
        let link = home.join(folder);
        let target = source.join(folder);
        if std::fs::read_link(&link).ok().as_deref() != Some(target.as_path()) || !target.is_dir() {
            return Err("Saved Kimi authentication is not shared; relaunch required".into());
        }
    }
    Ok(())
}

fn private_home(source: &Path, cwd: &Path, hooks: &HookContext) -> Result<PathBuf, String> {
    let home = hooks.spool.join("kimi-home");
    crate::platform::fs::private_dir(&home).map_err(|e| e.to_string())?;
    for name in ["device_id", "region"] {
        let path = source.join(name);
        if path.exists() {
            copy_private_file(&path, &home.join(name))?;
        }
    }
    share_auth(source, &home)?;
    // Carry only a trust decision the user already made for this exact cwd.
    // A fresh project still shows Kimi's native trust prompt.
    let trust_source = source.join("workspace-trust");
    if trust_source.is_dir() {
        for entry in std::fs::read_dir(&trust_source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 4096 {
                continue;
            }
            let bytes = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                continue;
            };
            if value.get("root").and_then(serde_json::Value::as_str) == cwd.to_str() {
                let trust_home = home.join("workspace-trust");
                crate::platform::fs::private_dir(&trust_home).map_err(|e| e.to_string())?;
                copy_private_file(&entry.path(), &trust_home.join(entry.file_name()))?;
            }
        }
    }
    let source_config =
        std::fs::read_to_string(source.join("config.toml")).map_err(|e| e.to_string())?;
    let mut config: toml::Value =
        toml::from_str(&source_config).map_err(|e| format!("Invalid Kimi config: {e}"))?;
    let table = config
        .as_table_mut()
        .ok_or("Kimi config root is not a table")?;
    table.remove("hooks"); // Never execute inherited global hooks in a Bus pane.
    let command = hook_command(hooks);
    let entries = hook_json::HookContract::for_provider(ProviderKind::Kimi)
        .events
        .iter()
        .copied()
        .map(|event| {
            let mut entry = toml::map::Map::new();
            entry.insert("event".into(), toml::Value::String(event.into()));
            entry.insert("command".into(), toml::Value::String(command.clone()));
            toml::Value::Table(entry)
        })
        .collect();
    table.insert("hooks".into(), toml::Value::Array(entries));
    let bytes = toml::to_string(&config).map_err(|e| e.to_string())?;
    hook_json::atomic_write(&home.join("config.toml"), bytes.as_bytes())
        .map_err(|e| e.to_string())?;
    write_tui_settings(&home)?;
    Ok(home)
}

fn write_tui_settings(home: &Path) -> Result<bool, String> {
    let path = home.join("tui.toml");
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| {
        !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 2 * 1024 * 1024
    }) {
        return Err("Kimi private TUI config is missing or invalid".into());
    }
    let mut config: toml::Value = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|_| "Kimi private TUI config is invalid")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(toml::map::Map::new())
        }
        Err(error) => return Err(error.to_string()),
    };
    let table = config
        .as_table_mut()
        .ok_or("Kimi private TUI config root is not a table")?;
    let mut changed = table
        .insert("cache_expiry_hint".into(), toml::Value::Boolean(false))
        .is_none_or(|value| value != toml::Value::Boolean(false));
    let upgrade = table
        .entry("upgrade")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
        .as_table_mut()
        .ok_or("Kimi private upgrade config is not a table")?;
    changed |= upgrade
        .insert("auto_install".into(), toml::Value::Boolean(false))
        .is_none_or(|value| value != toml::Value::Boolean(false));
    if changed {
        let bytes = toml::to_string(&config).map_err(|e| e.to_string())?;
        hook_json::atomic_write(&path, bytes.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

fn hook_command(hooks: &HookContext) -> String {
    let binary = crate::platform::remote_reattach_program(&hooks.binary.to_string_lossy());
    format!("{binary} --bus-callback kimi-hook")
}

pub(crate) fn rebind_hooks(hooks: &HookContext) -> Result<bool, String> {
    let home = saved_home(hooks)?;
    let path = home.join("config.toml");
    let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 2 * 1024 * 1024 {
        return Err("Kimi private hook config is missing or invalid".into());
    }
    let mut config: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|_| "Kimi private hook config is invalid")?;
    let hooks_table = config
        .as_table_mut()
        .and_then(|table| table.get_mut("hooks"))
        .and_then(toml::Value::as_array_mut)
        .ok_or("Kimi private hooks are missing")?;
    if hooks_table.len()
        != hook_json::HookContract::for_provider(ProviderKind::Kimi)
            .events
            .len()
    {
        return Err("Kimi private hooks changed; relaunch required".into());
    }
    let current = hook_command(hooks);
    let mut changed = false;
    for (entry, event) in hooks_table.iter_mut().zip(
        hook_json::HookContract::for_provider(ProviderKind::Kimi)
            .events
            .iter()
            .copied(),
    ) {
        let table = entry
            .as_table_mut()
            .ok_or("Kimi private hook entry is invalid")?;
        if table.len() != 2 || table.get("event").and_then(toml::Value::as_str) != Some(event) {
            return Err("Kimi private hooks changed; relaunch required".into());
        }
        let old = table
            .get("command")
            .and_then(toml::Value::as_str)
            .ok_or("Kimi private hook command is missing")?;
        if !old.ends_with(" --bus-callback kimi-hook") {
            return Err("Kimi private hook command changed; relaunch required".into());
        }
        if old != current {
            table.insert("command".into(), toml::Value::String(current.clone()));
            changed = true;
        }
    }
    if changed {
        let bytes = toml::to_string(&config).map_err(|e| e.to_string())?;
        hook_json::atomic_write(&path, bytes.as_bytes()).map_err(|e| e.to_string())?;
    }
    changed |= write_tui_settings(&home)?;
    Ok(changed)
}

pub(crate) fn option_arity(name: &str) -> Option<usize> {
    match name {
        "-S" | "--session" | "--agent-file" | "--agent" | "-m" | "--model" | "--output-format"
        | "-p" | "--prompt" | "--skills-dir" | "--add-dir" => Some(1),
        "-y" | "--yolo" | "--auto" | "--continue" | "-c" | "--plan" | "--help" | "-h"
        | "--version" | "-V" => Some(0),
        _ => None,
    }
}

/// Fresh launch and sessionless resume share this check. Attached
/// `-S<session>` / `-p<text>` and an external session selector are not an
/// interactive Bus-owned launch.
pub(crate) fn validate_owned_launch(args: &[String]) -> Result<Option<String>, String> {
    validate_args(args)?;
    take_adopted_session(args)
}

pub(crate) fn validate_args(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err("Kimi launch arguments contain NUL".into());
    }
    if let Some(index) = args.iter().position(|arg| arg == "--") {
        if index + 1 < args.len() {
            return Err("Kimi positional input conflicts with Bus interactive capture".into());
        }
    }
    if args
        .iter()
        .any(|arg| (arg.starts_with("-S") || arg.starts_with("-p")) && arg.len() > 2)
    {
        return Err(
            "Kimi attached session or prompt input conflicts with Bus interactive capture".into(),
        );
    }
    for index in common::argument_positions(ProviderKind::Kimi, args) {
        let arg = &args[index];
        let name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        if matches!(
            name,
            "-p" | "--prompt"
                | "--agent"
                | "--agent-file"
                | "--continue"
                | "-c"
                | "--plan"
                | "--help"
                | "-h"
                | "--version"
                | "-V"
        ) || !arg.starts_with('-')
        {
            return Err(format!(
                "Kimi launch argument {} conflicts with Bus interactive capture",
                arg
            ));
        }
    }
    Ok(())
}

pub(crate) fn take_adopted_session(args: &[String]) -> Result<Option<String>, String> {
    if common::argument_positions(ProviderKind::Kimi, args)
        .into_iter()
        .any(|i| {
            matches!(args[i].as_str(), "--session" | "-S")
                || args[i].starts_with("--session=")
                || args[i].starts_with("-S=")
                || args[i].starts_with("-S") && args[i].len() > 2
        })
    {
        return Err(
            "Adopting an external Kimi session is unsupported; add a fresh Bus agent".into(),
        );
    }
    Ok(None)
}

pub(crate) fn resume_args(session: &str) -> Vec<String> {
    vec!["--session".into(), session.into()]
}

pub(crate) fn resumable_options(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--" {
            break;
        }
        let option = args[index]
            .split_once('=')
            .map_or(args[index].as_str(), |(name, _)| name);
        let arity = option_arity(option);
        if !args[index].starts_with('-') {
            break;
        }
        if arity.is_none() {
            result.extend_from_slice(&args[index..]);
            break;
        }
        if option != "--agent-file" && option != "--session" && option != "-S" {
            result.push(args[index].clone());
            if arity == Some(1) && !args[index].contains('=') {
                if let Some(value) = args.get(index + 1) {
                    result.push(value.clone());
                }
            }
        }
        index += 1 + usize::from(arity == Some(1) && !args[index].contains('='));
    }
    result
}

pub(crate) fn bind_home(mut argv: Vec<String>, home: &Path) -> Result<Vec<String>, String> {
    let home = home
        .to_str()
        .filter(|value| home.is_absolute() && !value.chars().any(char::is_control))
        .ok_or("Invalid Kimi private home")?;
    argv.insert(0, format!("KIMI_CODE_HOME={home}"));
    argv.insert(0, "KIMI_DISABLE_OAUTH_LOCK=0".into());
    argv.insert(0, "/usr/bin/env".into());
    Ok(argv)
}

pub(crate) fn saved_home(hooks: &HookContext) -> Result<PathBuf, String> {
    let saved: HomeRecord =
        serde_json::from_value(hook_json::read_resume_json(&hooks.spool.join(HOME_FILE))?)
            .map_err(|_| "Saved Kimi home is invalid")?;
    if saved.path != hooks.spool.join("kimi-home") {
        return Err("Saved Kimi home is outside its callback root".into());
    }
    if home_record(&saved.path, &saved.source)? != saved {
        return Err("Saved Kimi home changed; relaunch required".into());
    }
    verify_shared_auth(&saved.path, &saved.source)?;
    Ok(saved.path)
}

pub(crate) fn saved_executable(hooks: &HookContext) -> Result<PathBuf, String> {
    let (path, saved): (PathBuf, Stamp) = serde_json::from_value(hook_json::read_resume_json(
        &hooks.spool.join(EXECUTABLE_FILE),
    )?)
    .map_err(|_| "Saved Kimi executable is invalid")?;
    if stamp(&path)? != saved {
        return Err("Saved Kimi executable changed; relaunch required".into());
    }
    Ok(path)
}

pub(crate) fn environment(hooks: &HookContext) -> Result<HashMap<String, String>, String> {
    let executable = saved_executable(hooks)?;
    let home = saved_home(hooks)?;
    environment_for(hooks, &executable, &home)
}

fn environment_for(
    hooks: &HookContext,
    executable: &Path,
    home: &Path,
) -> Result<HashMap<String, String>, String> {
    let directory = executable
        .parent()
        .ok_or("Kimi executable has no directory")?;
    let mut paths = vec![directory.to_path_buf()];
    if let Some(path) = crate::utils::env::path_var() {
        paths.extend(std::env::split_paths(&path).filter(|dir| dir != directory));
    }
    let path = std::env::join_paths(paths)
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|_| "Kimi PATH is not UTF-8")?;
    let mut env = hooks.env();
    env.insert("PATH".into(), path);
    env.insert("KIMI_CODE_HOME".into(), home.to_string_lossy().into_owned());
    env.insert("KIMI_DISABLE_OAUTH_LOCK".into(), "0".into());
    Ok(env)
}

pub(crate) fn validate_run_permissions(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "--auto")
        && args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--yolo" | "-y"))
    {
        return Err("Kimi --auto and --yolo cannot be combined".into());
    }
    Ok(())
}

pub(crate) fn prepare(
    spec: &LaunchSpec,
    initialize: impl FnOnce(&HookContext) -> Result<(), String>,
) -> Result<PreparedLaunch, String> {
    let cwd = common::canonical_directory(&spec.cwd)?;
    let mut options = common::parse_path_tokens(&spec.extra_args).map_err(|e| e.to_string())?;
    let adopted_session = validate_owned_launch(&options)?;
    validate_run_permissions(&options)?;
    if crate::utils::env::yolo_enabled() && options.iter().any(|arg| arg == "--auto") {
        return Err("Bus --yolo conflicts with Kimi --auto; choose one permission mode".into());
    }
    let saved_options = options.clone();
    if crate::utils::env::yolo_enabled()
        && !options
            .iter()
            .any(|arg| matches!(arg.as_str(), "--yolo" | "-y"))
    {
        options.insert(0, "--yolo".into());
    }
    let source = source_home()?;
    let executable = resolve_executable(&source)?;
    let home = private_home(&source, &cwd, &spec.hooks)?;
    initialize(&spec.hooks)?;
    for (name, bytes) in [
        (
            HOME_FILE,
            serde_json::to_vec(&home_record(&home, &source)?).map_err(|e| e.to_string())?,
        ),
        (
            EXECUTABLE_FILE,
            serde_json::to_vec(&(executable.clone(), stamp(&executable)?))
                .map_err(|e| e.to_string())?,
        ),
        (
            "launch-options.json",
            serde_json::to_vec(&saved_options).map_err(|e| e.to_string())?,
        ),
    ] {
        hook_json::atomic_write(&spec.hooks.spool.join(name), &bytes).map_err(|e| e.to_string())?;
    }
    let env = environment_for(&spec.hooks, &executable, &home)?;
    Ok(PreparedLaunch {
        cwd,
        args: options,
        adopted_session,
        env,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn private_home_shares_credential_and_refresh_lock_directories() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-shared-auth");
        let source = root.join("source");
        let spool = root.join("callbacks/launch");
        for folder in ["credentials", "oauth"] {
            std::fs::create_dir_all(source.join(folder)).unwrap();
            std::fs::write(source.join(folder).join("sentinel"), b"credential bytes").unwrap();
        }
        std::fs::write(source.join("config.toml"), b"").unwrap();
        let hooks = HookContext {
            binary: std::env::current_exe().unwrap(),
            spool,
            launch_id: "launch".into(),
        };
        let home = private_home(&source, &root, &hooks).unwrap();
        let tui = std::fs::read_to_string(home.join("tui.toml")).unwrap();
        assert!(tui.contains("cache_expiry_hint = false"));
        assert!(tui.contains("auto_install = false"));
        for folder in ["credentials", "oauth"] {
            let path = home.join(folder);
            assert!(std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read_link(&path).unwrap(), source.join(folder));
            std::fs::write(path.join("other"), b"updated").unwrap();
            assert_eq!(
                std::fs::read(source.join(folder).join("other")).unwrap(),
                b"updated"
            );
        }
        std::fs::write(home.join("credentials/replacement"), b"rotated").unwrap();
        std::fs::rename(
            home.join("credentials/replacement"),
            home.join("credentials/sentinel"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(source.join("credentials/sentinel")).unwrap(),
            b"rotated"
        );
        assert!(std::fs::symlink_metadata(home.join("credentials"))
            .unwrap()
            .file_type()
            .is_symlink());
        let mut directories = vec![home.clone()];
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    directories.push(entry.path());
                } else if kind.is_file() {
                    assert!(!std::fs::read(entry.path())
                        .unwrap()
                        .windows(b"credential bytes".len())
                        .any(|w| w == b"credential bytes"));
                }
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn kimi_persistent_home_disables_cache_expiry_hint() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-rebind-cache-hint");
        let source = root.join("source");
        let spool = root.join("callbacks/launch");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("config.toml"), b"").unwrap();
        let hooks = HookContext {
            binary: std::env::current_exe().unwrap(),
            spool,
            launch_id: "launch".into(),
        };
        let home = private_home(&source, &root, &hooks).unwrap();
        hook_json::atomic_write(
            &hooks.spool.join(HOME_FILE),
            &serde_json::to_vec(&home_record(&home, &source).unwrap()).unwrap(),
        )
        .unwrap();
        std::fs::write(
            home.join("tui.toml"),
            "cache_expiry_hint = true\ntheme = \"dark\"\n",
        )
        .unwrap();
        rebind_hooks(&hooks).unwrap();
        let tui = std::fs::read_to_string(home.join("tui.toml")).unwrap();
        assert!(tui.contains("cache_expiry_hint = false"), "{tui}");
        assert!(tui.contains("theme = \"dark\""), "{tui}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_home_rejects_legacy_copied_credentials() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-legacy-auth");
        let spool = root.join("callbacks/launch");
        let home = spool.join("kimi-home");
        crate::platform::fs::private_dir(&home.join("credentials")).unwrap();
        std::fs::write(home.join("credentials/kimi-code.json"), b"synthetic").unwrap();
        let meta = std::fs::symlink_metadata(&home).unwrap();
        let record = serde_json::json!({"path":home,"device":meta.dev(),"inode":meta.ino()});
        hook_json::atomic_write(
            &spool.join(HOME_FILE),
            &serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let hooks = HookContext {
            binary: std::env::current_exe().unwrap(),
            spool,
            launch_id: "launch".into(),
        };
        assert!(saved_home(&hooks).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_home_rejects_an_existing_private_credential_copy() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-existing-auth");
        let source = root.join("source");
        let spool = root.join("callbacks/launch");
        let credentials = spool.join("kimi-home/credentials");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&credentials).unwrap();
        std::fs::write(credentials.join("kimi-code.json"), b"synthetic").unwrap();
        let hooks = HookContext {
            binary: std::env::current_exe().unwrap(),
            spool,
            launch_id: "launch".into(),
        };
        assert!(private_home(&source, &root, &hooks).is_err());
        assert_eq!(
            std::fs::read(credentials.join("kimi-code.json")).unwrap(),
            b"synthetic"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_auth_can_be_prepared_before_first_login() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-first-login");
        let source = root.join("source");
        let home = root.join("private");
        std::fs::create_dir_all(&home).unwrap();
        share_auth(&source, &home).unwrap();
        verify_shared_auth(&home, &source).unwrap();
        for folder in ["credentials", "oauth"] {
            assert_eq!(
                std::fs::metadata(source.join(folder)).unwrap().mode() & 0o777,
                0o700
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_auth_follows_a_scratch_sources_existing_directory_links() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-linked-source");
        let source = root.join("source");
        let shared = root.join("shared");
        let home = root.join("private");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        for folder in ["credentials", "oauth"] {
            std::fs::create_dir_all(shared.join(folder)).unwrap();
            std::os::unix::fs::symlink(shared.join(folder), source.join(folder)).unwrap();
        }
        share_auth(&source, &home).unwrap();
        verify_shared_auth(&home, &source).unwrap();
        std::fs::write(home.join("credentials/token"), b"synthetic").unwrap();
        assert_eq!(
            std::fs::read(shared.join("credentials/token")).unwrap(),
            b"synthetic"
        );
        std::fs::create_dir(home.join("oauth/refresh.lock")).unwrap();
        assert!(shared.join("oauth/refresh.lock").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_auth_rejects_a_legacy_bus_home_as_source() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-nested-auth");
        let parent = root.join("callbacks/old-launch");
        let source = parent.join("kimi-home");
        let home = root.join("new-private-home");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(parent.join(HOME_FILE), b"old record").unwrap();
        assert!(share_auth(&source, &home).is_err());
        assert!(!home.join("credentials").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launch_and_resume_force_kimi_refresh_lock_on() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-lock-env");
        let hooks = HookContext {
            binary: std::env::current_exe().unwrap(),
            spool: root.join("spool"),
            launch_id: "launch".into(),
        };
        let environment = environment_for(&hooks, &root.join("bin/kimi"), &root).unwrap();
        assert_eq!(
            environment
                .get("KIMI_DISABLE_OAUTH_LOCK")
                .map(String::as_str),
            Some("0")
        );
        let argv = bind_home(vec!["kimi".into()], &root).unwrap();
        assert_eq!(argv[1], "KIMI_DISABLE_OAUTH_LOCK=0");
    }

    #[test]
    fn saved_home_rejects_a_replaced_private_directory() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-home-stamp");
        let spool = root.join("callbacks/launch");
        let home = spool.join("kimi-home");
        let source = root.join("source");
        crate::platform::fs::private_dir(&home).unwrap();
        share_auth(&source, &home).unwrap();
        let meta = std::fs::symlink_metadata(&home).unwrap();
        let record =
            serde_json::json!({"path":home,"source":source,"device":meta.dev(),"inode":meta.ino()});
        hook_json::atomic_write(
            &spool.join(HOME_FILE),
            &serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let hooks = HookContext {
            binary: std::env::current_exe().unwrap(),
            spool: spool.clone(),
            launch_id: "launch".into(),
        };
        assert_eq!(saved_home(&hooks).unwrap(), home);
        std::fs::remove_file(home.join("oauth")).unwrap();
        assert!(saved_home(&hooks).is_err());
        std::fs::create_dir(home.join("oauth")).unwrap();
        assert!(saved_home(&hooks).is_err());
        std::fs::rename(&home, spool.join("old-home")).unwrap();
        crate::platform::fs::private_dir(&home).unwrap();
        assert!(saved_home(&hooks).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
