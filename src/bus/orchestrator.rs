//! A MASTER orchestrator's Bus-owned working folder: its system prompt as
//! CLAUDE.md and AGENTS.md, the workflow-create skill, and the Bus docs the
//! prompt references. Every provider reads these files from its working
//! directory, so no launch flags are needed.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::io::{atomic_write, digest, private_dir};
use super::model::RoomId;

/// The default orchestrator system prompt, with `{{...}}` placeholders.
pub(crate) const DEFAULT_PROMPT: &str = include_str!("prompts/orchestrator.md");
const WORKFLOW_CREATE_SKILL: &str = include_str!("../../skills/workflow-create/SKILL.md");
/// The docs the prompt references, written under `<BUS_DATA_DIR>/docs/`.
const DOCS: &[(&str, &str)] = &[
    (
        "how-to-bus-cli.md",
        include_str!("../../docs/how-to-bus-cli.md"),
    ),
    (
        "orchestrator-guide.md",
        include_str!("../../docs/orchestrator-guide.md"),
    ),
    (
        "templates/workflow-template.md",
        include_str!("../../docs/templates/workflow-template.md"),
    ),
    (
        "workflows/pr-review-loop.md",
        include_str!("../../docs/workflows/pr-review-loop.md"),
    ),
    (
        "workflows/cross-repo-feature.md",
        include_str!("../../docs/workflows/cross-repo-feature.md"),
    ),
];
/// Claude Code reads CLAUDE.md; Codex and Cursor read AGENTS.md.
const PROMPT_FILES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];
/// Claude Code discovers `.claude/skills`; Codex and Cursor discover `.agents/skills`.
const SKILL_DIRS: [&str; 2] = [".claude/skills", ".agents/skills"];
const UNASSIGNED_ROOM_NAME: &str = "none yet (the human assigns one)";
const UNASSIGNED_ROOM_ID: &str = "ROOM";

/// What a MASTER agent is created with, beyond the plain agent input.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct OrchestratorSpec {
    pub room: Option<RoomId>,
    /// The system prompt text; `None` uses [`DEFAULT_PROMPT`]. `{{...}}`
    /// placeholders in it are filled in.
    pub system_prompt: Option<String>,
}

/// The values the prompt placeholders are filled with.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct PromptValues {
    pub room: Option<(String, RoomId)>,
    pub agent: String,
    pub docs: PathBuf,
}

pub(crate) fn default_folder(data_dir: &Path, agent_name: &str) -> PathBuf {
    data_dir.join("orchestrators").join(agent_name.trim())
}

pub(crate) fn docs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("docs")
}

pub(crate) fn fill(template: &str, values: &PromptValues) -> String {
    let (room_name, room_id) = match &values.room {
        Some((name, id)) => (name.clone(), id.0.to_string()),
        None => (UNASSIGNED_ROOM_NAME.into(), UNASSIGNED_ROOM_ID.into()),
    };
    template
        .replace("{{ROOM_NAME}}", &room_name)
        .replace("{{ROOM_ID}}", &room_id)
        .replace("{{AGENT_NAME}}", &values.agent)
        .replace("{{DOCS}}", &values.docs.to_string_lossy())
}

/// The template a submitted prompt came from: the text itself when it still has
/// placeholders, the default when it is the default filled in, otherwise none
/// (a custom prompt Bus cannot re-fill).
fn template_of(text: Option<&str>, values: &PromptValues) -> Option<String> {
    match text {
        None => Some(DEFAULT_PROMPT.into()),
        Some(text) if text.contains("{{") => Some(text.into()),
        Some(text) if text == fill(DEFAULT_PROMPT, values) => Some(DEFAULT_PROMPT.into()),
        Some(_) => None,
    }
}

/// What Bus wrote into a folder, so it can tell later whether the human edited it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Record {
    template: Option<String>,
    values: PromptValues,
    /// SHA-256 of each prompt file as Bus wrote it.
    hashes: Vec<(String, String)>,
}

fn record_path(data_dir: &Path, folder: &Path) -> PathBuf {
    data_dir.join("orchestrator-prompts").join(format!(
        "{}.json",
        digest(folder.to_string_lossy().as_bytes())
    ))
}

fn load_record(data_dir: &Path, folder: &Path) -> Option<Record> {
    let bytes = std::fs::read(record_path(data_dir, folder)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_record(data_dir: &Path, folder: &Path, record: &Record) -> Result<(), String> {
    let path = record_path(data_dir, folder);
    private_dir(path.parent().expect("record has a parent")).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    atomic_write(&path, &bytes).map_err(|e| e.to_string())
}

fn file_hash(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| digest(&bytes))
}

fn written_by_bus(record: Option<&Record>, folder: &Path, file: &str) -> bool {
    let current = file_hash(&folder.join(file));
    record.is_some_and(|record| {
        record
            .hashes
            .iter()
            .any(|(name, hash)| name == file && Some(hash) == current.as_ref())
    })
}

/// Writes the embedded Bus docs under `<data_dir>/docs/` and returns that folder.
pub(crate) fn write_docs(data_dir: &Path) -> Result<PathBuf, String> {
    let root = docs_dir(data_dir);
    for (name, text) in DOCS {
        let path = root.join(name);
        private_dir(path.parent().expect("doc has a parent")).map_err(|e| e.to_string())?;
        atomic_write(&path, text.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(root)
}

/// Creates the default folder owner-only when it is missing; a custom folder
/// must already exist.
pub(crate) fn ensure_folder(
    data_dir: &Path,
    folder: &Path,
    agent_name: &str,
) -> Result<(), String> {
    if folder == default_folder(data_dir, agent_name) {
        private_dir(folder).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Rejects an agent name that would not be one folder under `orchestrators/`.
pub(crate) fn check_folder_name(agent_name: &str) -> Result<(), String> {
    let name = agent_name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(format!(
            "Orchestrator name {name:?} cannot name its working folder; choose another name or give a PWD"
        ));
    }
    Ok(())
}

/// Writes the prompt files and skills into `folder`. Refuses to replace a
/// CLAUDE.md or AGENTS.md that Bus did not write there.
pub(crate) fn write_folder(
    data_dir: &Path,
    folder: &Path,
    system_prompt: Option<&str>,
    values: &PromptValues,
) -> Result<(), String> {
    let record = load_record(data_dir, folder);
    for file in PROMPT_FILES {
        if folder.join(file).exists() && !written_by_bus(record.as_ref(), folder, file) {
            return Err(format!(
                "{} already exists and was not written by Bus; remove it or choose another PWD",
                folder.join(file).display()
            ));
        }
    }
    let template = template_of(system_prompt, values);
    let text = match (&template, system_prompt) {
        (Some(template), _) => fill(template, values),
        (None, Some(text)) => text.to_owned(),
        (None, None) => unreachable!("no text always uses the default template"),
    };
    for dir in SKILL_DIRS {
        let path = folder.join(dir).join("workflow-create/SKILL.md");
        private_dir(path.parent().expect("skill has a parent")).map_err(|e| e.to_string())?;
        atomic_write(&path, WORKFLOW_CREATE_SKILL.as_bytes()).map_err(|e| e.to_string())?;
    }
    let hashes = write_prompts(folder, &text, &PROMPT_FILES)?;
    save_record(
        data_dir,
        folder,
        &Record {
            template,
            values: values.clone(),
            hashes,
        },
    )
}

fn write_prompts(
    folder: &Path,
    text: &str,
    files: &[&str],
) -> Result<Vec<(String, String)>, String> {
    files
        .iter()
        .map(|file| {
            atomic_write(&folder.join(file), text.as_bytes()).map_err(|e| e.to_string())?;
            Ok(((*file).to_owned(), digest(text.as_bytes())))
        })
        .collect()
}

/// The outcome of re-filling the prompt for a new room.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Retarget {
    pub rewritten: Vec<PathBuf>,
    /// Files left alone because the human edited them or the prompt was custom.
    pub kept: Vec<PathBuf>,
}

impl Retarget {
    pub(crate) fn notice(&self) -> Option<String> {
        if self.kept.is_empty() {
            return None;
        }
        let files: Vec<_> = self.kept.iter().map(|p| p.display().to_string()).collect();
        Some(format!(
            "Kept {} unchanged: edited since Bus wrote it, or a custom prompt; update its room by hand.",
            files.join(" and ")
        ))
    }
}

/// Re-fills the room placeholders after a reassignment, file by file, only
/// where the file is still exactly what Bus wrote. Folders Bus never wrote
/// are left alone.
pub(crate) fn retarget(
    data_dir: &Path,
    folder: &Path,
    room: Option<(String, RoomId)>,
) -> Result<Retarget, String> {
    let Some(mut record) = load_record(data_dir, folder) else {
        return Ok(Retarget::default());
    };
    let mut outcome = Retarget::default();
    record.values.room = room;
    let Some(template) = record.template.clone() else {
        outcome.kept = PROMPT_FILES.iter().map(|f| folder.join(f)).collect();
        return Ok(outcome);
    };
    let text = fill(&template, &record.values);
    let mut hashes = Vec::new();
    for file in PROMPT_FILES {
        if written_by_bus(Some(&record), folder, file) {
            hashes.extend(write_prompts(folder, &text, &[file])?);
            outcome.rewritten.push(folder.join(file));
        } else {
            outcome.kept.push(folder.join(file));
        }
    }
    // Keep the old hash for a file the human edited, so it stays "edited".
    record
        .hashes
        .retain(|(name, _)| !hashes.iter().any(|(n, _)| n == name));
    record.hashes.extend(hashes);
    save_record(data_dir, folder, &record)?;
    Ok(outcome)
}

#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod tests;
