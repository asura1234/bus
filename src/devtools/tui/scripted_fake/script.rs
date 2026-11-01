//! Script parsing and one-shot turn selection for the generic fake agent.
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Script {
    version: u32,
    turns: Vec<Turn>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    contains: String,
    actions: Vec<Action>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Reply {
        text: String,
    },
    Fail,
    Sleep {
        ms: u64,
    },
    Dialog {
        title: String,
        options: Vec<String>,
    },
    Screen {
        id: String,
        screen: String,
        input: super::screens::ScreenInput,
    },
    Send {
        room: String,
        to: String,
        text: String,
    },
    Crash {
        code: i32,
    },
}

pub(super) struct Scenario {
    script: Script,
    consumed: Vec<usize>,
    state: PathBuf,
}

impl Scenario {
    pub(super) fn load(path: &Path, state: PathBuf) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Cannot read fake script: {e}"))?;
        let script: Script =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid fake script: {e}"))?;
        if script.version != 1 {
            return Err(format!(
                "Unsupported fake script version {}",
                script.version
            ));
        }
        for turn in &script.turns {
            if turn.contains.is_empty() || turn.actions.is_empty() {
                return Err("Fake turns need a nonempty match and at least one action".into());
            }
            for (index, action) in turn.actions.iter().enumerate() {
                let terminal = matches!(
                    action,
                    Action::Reply { .. } | Action::Fail | Action::Crash { .. }
                );
                if terminal != (index + 1 == turn.actions.len()) {
                    return Err(
                        "Each fake turn must end with exactly one reply, fail or crash".into(),
                    );
                }
                if let Action::Dialog { options, .. } = action {
                    if options.is_empty() || options.iter().any(|option| option.trim().is_empty()) {
                        return Err("Fake dialogs require nonempty options".into());
                    }
                }
                if let Action::Screen { id, screen, input } = action {
                    if id.trim().is_empty() || screen.trim().is_empty() {
                        return Err("Fake screens need an id and visible content".into());
                    }
                    input.validate(screen)?;
                }
            }
        }
        let consumed = match std::fs::read(&state) {
            Ok(bytes) => serde_json::from_slice::<Vec<usize>>(&bytes)
                .map_err(|e| format!("Invalid fake state: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("Cannot read fake state: {e}")),
        };
        if consumed.iter().any(|&index| index >= script.turns.len()) {
            return Err("Fake state does not match script".into());
        }
        Ok(Self {
            script,
            consumed,
            state,
        })
    }

    pub(super) fn actions(&mut self, prompt: &str) -> Result<Vec<Action>, String> {
        let Some((index, turn)) = self.script.turns.iter().enumerate().find(|(index, turn)| {
            !self.consumed.contains(index) && prompt.contains(&turn.contains)
        }) else {
            return Ok(vec![Action::Reply {
                text: format!("fake reply: {prompt}"),
            }]);
        };
        let actions = turn.actions.clone();
        self.consumed.push(index);
        let bytes = serde_json::to_vec(&self.consumed).map_err(|e| e.to_string())?;
        crate::agents::providers::hook_json::atomic_write(&self.state, &bytes)
            .map_err(|e| format!("Cannot save fake state: {e}"))?;
        Ok(actions)
    }
}
