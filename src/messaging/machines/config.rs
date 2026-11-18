use crate::messaging::storage::{io, sessions};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MachineTarget {
    pub label: String,
    pub target: String,
    pub enabled: bool,
}

pub(crate) fn validate_target(target: &str) -> Result<(), String> {
    if target.is_empty()
        || target.starts_with('-')
        || target.len() > 255
        || !target.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'_' | b'-' | b'@' | b':' | b'[' | b']')
        })
    {
        return Err("SSH target must be a host, user@host, or an SSH config alias (no options or shell syntax)".into());
    }
    Ok(())
}

fn validate(targets: &[MachineTarget]) -> Result<(), String> {
    let mut labels = std::collections::HashSet::new();
    for target in targets {
        if target.label.trim().is_empty()
            || target.label.len() > 80
            || target.label.chars().any(char::is_control)
            || target.label.eq_ignore_ascii_case("local")
        {
            return Err(
                "Machine label must be 1-80 printable characters and cannot be Local".into(),
            );
        }
        if !labels.insert(&target.label) {
            return Err(format!("Duplicate machine label {:?}", target.label));
        }
        validate_target(&target.target)?;
    }
    Ok(())
}

fn path() -> Result<PathBuf, String> {
    Ok(sessions::default_base_dir()?.join("machines.json"))
}

fn read(path: &Path) -> Result<Vec<MachineTarget>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Cannot read SSH targets: {error}")),
    };
    let targets: Vec<MachineTarget> =
        serde_json::from_slice(&bytes).map_err(|error| format!("Invalid SSH targets: {error}"))?;
    validate(&targets)?;
    Ok(targets)
}

pub(crate) fn load_targets() -> Result<Vec<MachineTarget>, String> {
    read(&path()?)
}

pub(crate) fn update_targets(
    update: impl FnOnce(&mut Vec<MachineTarget>) -> Result<(), String>,
) -> Result<(), String> {
    update_at(&path()?, update)
}

fn update_at(
    path: &Path,
    update: impl FnOnce(&mut Vec<MachineTarget>) -> Result<(), String>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("SSH targets have no parent directory")?;
    io::private_dir(parent).map_err(|error| error.to_string())?;
    let _lease =
        io::append_lock(&parent.join("machines.lock")).map_err(|error| error.to_string())?;
    let mut targets = read(path)?;
    update(&mut targets)?;
    validate(&targets)?;
    let bytes = serde_json::to_vec_pretty(&targets).map_err(|error| error.to_string())?;
    io::atomic_write(path, &bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_targets_contain_only_connection_preferences() {
        let dir = crate::utils::test_temp::unique_temp_path("machine-targets");
        let path = dir.join("machines.json");
        update_at(&path, |targets| {
            targets.push(MachineTarget {
                label: "Build Mac".into(),
                target: "dev@mac".into(),
                enabled: true,
            });
            Ok(())
        })
        .unwrap();
        let targets = read(&path).unwrap();
        assert_eq!(targets.len(), 1);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value[0].as_object().unwrap().len(), 3);
        update_at(&path, |targets| {
            targets[0].enabled = false;
            Ok(())
        })
        .unwrap();
        assert!(!read(&path).unwrap()[0].enabled);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn targets_reject_options_shell_fragments_and_private_fields() {
        for target in [
            "",
            "-oProxyCommand=bad",
            "host;touch bad",
            "host\nexec",
            "$(id)",
            "user@host command",
        ] {
            assert!(validate_target(target).is_err(), "{target}");
        }
        for target in ["build-mac", "dev@example.net", "dev@[::1]", "::1"] {
            assert!(validate_target(target).is_ok(), "{target}");
        }
        assert!(serde_json::from_str::<MachineTarget>(
            r#"{"label":"mac","target":"mac","enabled":true,"password":"secret"}"#
        )
        .is_err());
    }
}
