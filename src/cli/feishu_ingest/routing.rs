//! Reuse routing snapshots so unmatched receipts do not multiply history IO.
use super::Target;
use crate::messaging::{
    model::feishu::{FeishuState, IncomingReply},
    storage::state_store::JsonStore,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

type Sessions = Vec<(PathBuf, FeishuState)>;
pub(super) type ReadResult = Result<Option<FeishuState>, String>;

pub(super) fn read(root: &Path) -> ReadResult {
    JsonStore::new(root.join("state.json"))
        .load()
        .map(|state| state.map(|state| state.feishu))
        .map_err(|error| format!("Cannot inspect Bus routing state: {error}"))
}

#[derive(Default)]
pub(super) struct Routes {
    sessions: Option<Result<Sessions, String>>,
}

impl Routes {
    pub(super) fn select(
        &mut self,
        root: &Path,
        input: &IncomingReply,
        read: &mut impl FnMut(&Path) -> ReadResult,
    ) -> Result<Target, String> {
        let sessions = self
            .sessions
            .get_or_insert_with(|| {
                let entries = std::fs::read_dir(root.join("sessions"))
                    .map_err(|_| "Feishu sink instance is neither a session nor a registry")?;
                let mut sessions = Vec::new();
                for entry in entries {
                    let entry = entry.map_err(|_| "Cannot inspect Bus session registry")?;
                    if !entry
                        .file_type()
                        .map_err(|_| "Cannot inspect Bus registry entry")?
                        .is_dir()
                    {
                        continue;
                    }
                    let root = std::fs::canonicalize(entry.path())
                        .map_err(|_| "Cannot resolve Bus session root")?;
                    if let Some(state) = read(&root)? {
                        sessions.push((root, state));
                    }
                }
                Ok(sessions)
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let mut receipts = BTreeSet::new();
        let mut references = BTreeSet::new();
        for (root, state) in sessions {
            if state.inbox.contains_key(&input.key()) {
                receipts.insert(root.clone());
            }
            if state.resolve(input).is_ok() || references_session(state, input) {
                references.insert(root.clone());
            }
        }
        let candidates = if receipts.is_empty() {
            references
        } else {
            receipts
        };
        if candidates.len() != 1 {
            return Ok(Target::Parked(if candidates.is_empty() {
                "Unknown Feishu reply; no mapped Bus session"
            } else {
                "Ambiguous Feishu reply; multiple Bus sessions match"
            }));
        }
        candidates
            .into_iter()
            .next()
            .map(Target::Session)
            .ok_or_else(|| "Unknown Feishu reply".into())
    }

    pub(super) fn refresh(
        &mut self,
        root: &Path,
        read: &mut impl FnMut(&Path) -> ReadResult,
    ) -> Result<(), String> {
        if let Some(Ok(sessions)) = &mut self.sessions {
            match read(root)? {
                Some(state) => {
                    if let Some((_, saved)) = sessions.iter_mut().find(|(path, _)| path == root) {
                        *saved = state;
                    } else {
                        sessions.push((root.to_owned(), state));
                    }
                }
                None => sessions.retain(|(path, _)| path != root),
            }
        }
        Ok(())
    }
}

pub(super) fn has_references(input: &IncomingReply) -> bool {
    input.reply_to.is_some() || input.root_id.is_some() || codes(input).next().is_some()
}

fn codes(input: &IncomingReply) -> impl Iterator<Item = &str> {
    input.text.split_whitespace().filter_map(|word| {
        word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':')
            .strip_prefix("bus:")
    })
}

/// Locally conflicting evidence still prevents a false unique match elsewhere.
fn references_session(state: &FeishuState, input: &IncomingReply) -> bool {
    input
        .reply_to
        .iter()
        .chain(input.root_id.iter())
        .any(|message| {
            state
                .outbox
                .values()
                .any(|record| record.message_id.as_ref() == Some(message))
                || state
                    .inbox
                    .contains_key(&format!("{}:{message}", input.app_id))
        })
        || codes(input).any(|code| state.outbox.contains_key(code))
}
