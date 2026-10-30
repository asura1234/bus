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
const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "TurnStarted",
    "Stop",
    "StopFailure",
    "Interrupt",
];

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
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn home_record(path: &Path) -> Result<HomeRecord, String> {
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
    if let Some(path) = std::env::var_os("PATH") {
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

fn private_home(source: &Path, cwd: &Path, hooks: &HookContext) -> Result<PathBuf, String> {
    let home = hooks.spool.join("kimi-home");
    crate::messaging::storage::io::private_dir(&home).map_err(|e| e.to_string())?;
    for name in ["device_id", "region"] {
        let path = source.join(name);
        if path.exists() {
            copy_private_file(&path, &home.join(name))?;
        }
    }
    for folder in ["oauth", "credentials"] {
        if source.join(folder).is_dir() {
            crate::messaging::storage::io::private_dir(&home.join(folder))
                .map_err(|e| e.to_string())?;
            for entry in std::fs::read_dir(source.join(folder)).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                copy_private_file(&entry.path(), &home.join(folder).join(entry.file_name()))?;
            }
        }
    }
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
                crate::messaging::storage::io::private_dir(&trust_home)
                    .map_err(|e| e.to_string())?;
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
    let entries = HOOK_EVENTS
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
    hook_json::atomic_write(&home.join("tui.toml"), b"[upgrade]\nauto_install = false\n")
        .map_err(|e| e.to_string())?;
    Ok(home)
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
    if hooks_table.len() != HOOK_EVENTS.len() {
        return Err("Kimi private hooks changed; relaunch required".into());
    }
    let current = hook_command(hooks);
    let mut changed = false;
    for (entry, event) in hooks_table.iter_mut().zip(HOOK_EVENTS.iter().copied()) {
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

pub(crate) fn validate_args(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err("Kimi launch arguments contain NUL".into());
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

pub(crate) fn take_adopted_session(args: &mut Vec<String>) -> Result<Option<String>, String> {
    if common::argument_positions(ProviderKind::Kimi, args)
        .into_iter()
        .any(|i| {
            matches!(args[i].as_str(), "--session" | "-S")
                || args[i].starts_with("--session=")
                || args[i].starts_with("-S=")
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
    if home_record(&saved.path)? != saved {
        return Err("Saved Kimi home changed; relaunch required".into());
    }
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
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).filter(|dir| dir != directory));
    }
    let path = std::env::join_paths(paths)
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|_| "Kimi PATH is not UTF-8")?;
    let mut env = hooks.env();
    env.insert("PATH".into(), path);
    env.insert("KIMI_CODE_HOME".into(), home.to_string_lossy().into_owned());
    Ok(env)
}

pub(crate) fn prepare(
    spec: &LaunchSpec,
    initialize: impl FnOnce(&HookContext) -> Result<(), String>,
) -> Result<PreparedLaunch, String> {
    let cwd = common::canonical_directory(&spec.cwd)?;
    let mut options = common::parse_path_tokens(&spec.extra_args).map_err(|e| e.to_string())?;
    validate_args(&options)?;
    let adopted_session = take_adopted_session(&mut options)?;
    if options.iter().any(|arg| arg == "--auto")
        && options
            .iter()
            .any(|arg| matches!(arg.as_str(), "--yolo" | "-y"))
    {
        return Err("Kimi --auto and --yolo cannot be combined".into());
    }
    if crate::utils::env::yolo_enabled() && options.iter().any(|arg| arg == "--auto") {
        return Err("Bus --yolo conflicts with Kimi --auto; choose one permission mode".into());
    }
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
            serde_json::to_vec(&home_record(&home)?).map_err(|e| e.to_string())?,
        ),
        (
            EXECUTABLE_FILE,
            serde_json::to_vec(&(executable.clone(), stamp(&executable)?))
                .map_err(|e| e.to_string())?,
        ),
        (
            "launch-options.json",
            serde_json::to_vec(&options).map_err(|e| e.to_string())?,
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
    fn saved_home_rejects_a_replaced_private_directory() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-home-stamp");
        let spool = root.join("callbacks/launch");
        let home = spool.join("kimi-home");
        crate::messaging::storage::io::private_dir(&home).unwrap();
        let meta = std::fs::symlink_metadata(&home).unwrap();
        let record = serde_json::json!({"path":home,"device":meta.dev(),"inode":meta.ino()});
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
        std::fs::rename(&home, spool.join("old-home")).unwrap();
        crate::messaging::storage::io::private_dir(&home).unwrap();
        assert!(saved_home(&hooks).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
