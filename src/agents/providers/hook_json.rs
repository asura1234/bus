//! Common observation-hook definition and launch-capture contracts.
//!
//! Exact hook bytes, file installation, ownership-preserving merge and resume
//! validation are shared by all three provider harnesses.

use super::ProviderKind;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Caller-supplied capture facts. Constructing this is not proof of ownership:
/// a cold resume must validate its saved launch/session/spool before using it.
/// The spool owner initializes its manifest; this DTO does not create files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HookContext {
    pub(crate) binary: PathBuf,
    pub(crate) spool: PathBuf,
    pub(crate) launch_id: String,
}

impl HookContext {
    /// Environment inherited by the provider process and its callbacks.
    pub(crate) fn env(&self) -> HashMap<String, String> {
        HashMap::from([
            ("BUS_LAUNCH_ID".into(), self.launch_id.clone()),
            (
                "BUS_CALLBACK_DIR".into(),
                self.spool.to_string_lossy().into_owned(),
            ),
        ])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HookEntryShape {
    /// Claude and Codex: an event entry containing one command hook.
    NestedCommands,
    /// Cursor: an event entry containing the command directly.
    Command,
}

/// Exact Bus-owned observation-hook definition, including event order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HookContract {
    pub(crate) adapter: &'static str,
    pub(crate) events: &'static [&'static str],
    pub(crate) entry_shape: HookEntryShape,
    /// Cursor supplies version 1 only when the document has no version.
    pub(crate) default_version: Option<u64>,
}

impl HookContract {
    pub(crate) const fn for_provider(provider: ProviderKind) -> Self {
        match provider {
            ProviderKind::Codex => Self {
                adapter: "codex-hook",
                events: &["SessionStart", "UserPromptSubmit", "Stop"],
                entry_shape: HookEntryShape::NestedCommands,
                default_version: None,
            },
            ProviderKind::ClaudeCode => Self {
                adapter: "claude-hook",
                events: &["SessionStart", "UserPromptSubmit", "Stop", "StopFailure"],
                entry_shape: HookEntryShape::NestedCommands,
                default_version: None,
            },
            ProviderKind::Cursor => Self {
                adapter: "cursor-hook",
                events: &[
                    "sessionStart",
                    "beforeSubmitPrompt",
                    "afterAgentResponse",
                    "stop",
                ],
                entry_shape: HookEntryShape::Command,
                default_version: Some(1),
            },
        }
    }

    /// Hook definitions are shell commands; provider launch argv is separate.
    /// Keep the platform's existing quoting and the live callback CLI intact.
    pub(crate) fn command(self, binary: &Path) -> String {
        let quoted = crate::platform::remote_reattach_program(&binary.to_string_lossy());
        format!("{quoted} --bus-callback {}", self.adapter)
    }

    /// The exact JSON shape used to recognize and replace Bus-owned entries.
    /// User hooks and lookalikes must retain the existing fail-closed rules.
    pub(crate) fn entry(self, command: &str) -> Value {
        match self.entry_shape {
            HookEntryShape::NestedCommands => {
                json!({"hooks":[{"type":"command","command":command,"timeout":5}]})
            }
            HookEntryShape::Command => json!({"command":command}),
        }
    }
}

#[path = "hook_json/files.rs"]
mod files;
pub(crate) use files::atomic_write;
#[cfg(test)]
pub(crate) use files::private_dir;

fn hook_entries(contract: HookContract, binary: &Path) -> Vec<(&'static str, Value)> {
    let command = contract.command(binary);
    contract
        .events
        .iter()
        .map(|event| (*event, contract.entry(&command)))
        .collect()
}

fn is_owned_entry(entry: &Value, contract: HookContract) -> bool {
    let command = if contract.entry_shape == HookEntryShape::Command {
        &entry["command"]
    } else {
        &entry["hooks"][0]["command"]
    };
    let Some(command) = command.as_str() else {
        return false;
    };
    let Some(program) = command.strip_suffix(&format!(" --bus-callback {}", contract.adapter))
    else {
        return false;
    };
    let path = match program
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        Some(quoted) => quoted.replace("'\\''", "'"),
        None => program.to_owned(),
    };
    !path.is_empty()
        && crate::platform::remote_reattach_program(&path) == program
        && *entry == contract.entry(command)
}

pub(crate) fn merge_hooks(
    mut document: Value,
    contract: HookContract,
    binary: &Path,
) -> Result<Value, String> {
    let root = document
        .as_object_mut()
        .ok_or("Hook config must be a JSON object")?;
    if let Some(version) = contract.default_version {
        root.entry("version").or_insert(json!(version));
    }
    let hooks = root
        .entry("hooks")
        .or_insert(json!({}))
        .as_object_mut()
        .ok_or("hooks must be a JSON object")?;
    for (event, owned) in hook_entries(contract, binary) {
        let group = hooks
            .entry(event)
            .or_insert(json!([]))
            .as_array_mut()
            .ok_or("Hook event must contain an array")?;
        // Entries from another Bus executable move to this one, in place.
        let mut placed = false;
        group.retain_mut(|entry| {
            if !is_owned_entry(entry, contract) {
                return true;
            }
            if placed {
                return false;
            }
            placed = true;
            *entry = owned.clone();
            true
        });
        if placed {
            continue;
        }
        if group
            .iter()
            .any(|entry| entry.to_string().contains("--bus-callback"))
        {
            return Err(format!(
                "Conflicting Bus hook in {event}; inspect/remove that owned entry before setup"
            ));
        }
        group.push(owned);
    }
    Ok(document)
}

pub(crate) fn rebind_owned_hooks(
    path: &Path,
    contract: HookContract,
    binary: &Path,
) -> Result<(), String> {
    let document: Value = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let owned = hook_entries(contract, binary).iter().all(|(event, _)| {
        document["hooks"][*event]
            .as_array()
            .is_some_and(|group| group.iter().any(|entry| is_owned_entry(entry, contract)))
    });
    if !owned {
        return Err("Bus hooks are missing; existing configuration was kept".into());
    }
    install_hooks(path, contract, binary)
}

pub(crate) fn install_hooks(
    path: &Path,
    contract: HookContract,
    binary: &Path,
) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing hook config parent")?;
    // A test binary written into a real project's hooks silently captures the
    // callbacks of every live agent there; tests must use a temporary project.
    #[cfg(test)]
    assert!(
        is_temporary(path),
        "tests must not install hooks outside the temp directory: {}",
        path.display()
    );
    files::private_dir(parent).map_err(|e| e.to_string())?;
    let _lease = files::lock(&parent.join(".bus-hooks.lock")).map_err(|e| e.to_string())?;
    let previous = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let document = match previous.as_ref() {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|e| e.to_string())?,
        None => json!({}),
    };
    let merged = merge_hooks(document, contract, binary)?;
    let current = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    if current != previous {
        return Err("Hook config changed during setup; retry confirmation".into());
    }
    atomic_write(
        path,
        &serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
fn is_temporary(path: &Path) -> bool {
    let temp = std::env::temp_dir();
    let canonical = temp.canonicalize().unwrap_or_else(|_| temp.clone());
    path.starts_with(&temp) || path.starts_with(&canonical)
}

pub(crate) fn read_resume_json(path: &Path) -> Result<Value, String> {
    use std::io::Read;
    if !path.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err("Bus resume capture configuration is missing".into());
    }
    let file =
        std::fs::File::open(path).map_err(|_| "Bus resume capture configuration is unreadable")?;
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Bus resume capture configuration is unreadable")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Bus resume capture configuration is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Bus resume capture configuration is invalid".into())
}

fn resume_entry_matches(entry: &Value, shape: HookEntryShape, command: &str) -> bool {
    match shape {
        HookEntryShape::Command => entry.get("command").and_then(Value::as_str) == Some(command),
        HookEntryShape::NestedCommands => {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook.get("type").and_then(Value::as_str) == Some("command")
                            && hook.get("command").and_then(Value::as_str) == Some(command)
                    })
                })
        }
    }
}

pub(crate) fn validate_resume_hooks(
    path: &Path,
    contract: HookContract,
    binary: &Path,
) -> Result<(), String> {
    let document = read_resume_json(path)?;
    let command = contract.command(binary);
    let complete = document.get("disableAllHooks").and_then(Value::as_bool) != Some(true)
        && contract.events.iter().all(|event| {
            document
                .get("hooks")
                .and_then(|hooks| hooks.get(event))
                .and_then(Value::as_array)
                .is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| resume_entry_matches(entry, contract.entry_shape, &command))
                })
        });
    if !complete {
        return Err(
            "Bus resume hooks are missing or changed; existing configuration was kept".into(),
        );
    }
    Ok(())
}

pub(crate) fn resume_hooks(
    path: &Path,
    contract: HookContract,
    binary: &Path,
) -> Result<bool, String> {
    if validate_resume_hooks(path, contract, binary).is_ok() {
        return Ok(false);
    }
    // Rebind only existing exact Bus-owned entries. Do not create missing
    // capture configuration or reinterpret user entries as owned hooks.
    rebind_owned_hooks(path, contract, binary)
        .map_err(|_| "Bus resume hooks are missing or changed; existing configuration was kept")?;
    validate_resume_hooks(path, contract, binary)?;
    Ok(true)
}
