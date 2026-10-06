//! A MASTER orchestrator's system prompt and the Bus docs it references. The
//! prompt is a Bus-owned file in the launch's callback folder, delivered with
//! each provider's own launch option; nothing is written into the agent's PWD.
use std::path::{Path, PathBuf};

use super::io::{atomic_write, private_dir};
use super::model::{Provider, RoomId};

/// The default orchestrator system prompt, with `{{...}}` placeholders.
pub(crate) const DEFAULT_PROMPT: &str = include_str!("prompts/orchestrator.md");
const WORKFLOW_CREATE: &str = include_str!("../../skills/workflow-create/SKILL.md");
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
        "orchestrator-rules.md",
        include_str!("../../docs/orchestrator-rules.md"),
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
/// The prompt file in a launch's callback folder; resumes deliver it again.
pub(crate) const PROMPT_FILE: &str = "system-prompt.md";
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PromptValues {
    pub room: Option<(String, RoomId)>,
    pub agent: String,
    pub docs: PathBuf,
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

/// The workflow-create guide as a doc: the repository skill without its
/// frontmatter, which only skill loaders read.
fn workflow_create_doc() -> &'static str {
    WORKFLOW_CREATE
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(WORKFLOW_CREATE, |(_, body)| body.trim_start())
}

/// Writes the embedded Bus docs under `<data_dir>/docs/` and returns that folder.
pub(crate) fn write_docs(data_dir: &Path) -> Result<PathBuf, String> {
    let root = docs_dir(data_dir);
    let docs = DOCS
        .iter()
        .copied()
        .chain([("workflow-create.md", workflow_create_doc())]);
    for (name, text) in docs {
        let path = root.join(name);
        private_dir(path.parent().expect("doc has a parent")).map_err(|e| e.to_string())?;
        atomic_write(&path, text.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(root)
}

/// Writes the prompt into a launch's callback folder.
pub(crate) fn write_prompt(spool: &Path, text: &str) -> Result<PathBuf, String> {
    let path = spool.join(PROMPT_FILE);
    atomic_write(&path, text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The Bus-owned launch arguments that deliver the prompt, or `None` when the
/// provider cannot take it at launch: Bus then sends it as the first message.
/// `adopted` means the launch resumes a session that started outside Bus.
pub(crate) fn prompt_args(
    provider: Provider,
    path: &Path,
    adopted: bool,
) -> Result<Option<Vec<String>>, String> {
    Ok(match provider {
        // Appends to Claude Code's default prompt; a file keeps the typed command
        // short. Claude records a session's system prompt on its first request
        // and replays that record on resume, so an adopted session must render
        // the prompt fresh to see the appended file.
        Provider::ClaudeCode => {
            let mut args = Vec::new();
            if adopted {
                args.extend(["--system-prompt-snapshot".into(), "off".into()]);
            }
            args.extend([
                "--append-system-prompt-file".into(),
                path.to_string_lossy().into_owned(),
            ]);
            Some(args)
        }
        // A resumed Codex thread keeps the developer_instructions it started
        // with and ignores new ones, so an adopted thread gets a message.
        Provider::Codex if adopted => None,
        // Codex adds developer_instructions as a developer message beside its
        // base instructions. It has no file variant, and Bus types the launch
        // command into the pane's shell, where a prompt-sized line can take
        // minutes to echo before Enter. So the message only points at the file,
        // as a one-line TOML string (JSON string escapes are valid TOML).
        Provider::Codex => {
            let text = format!(
                "You are a Bus orchestrator. Your system prompt is the file {}. \
                 Read all of it before you act on any message, and follow it for the \
                 whole session.",
                path.display()
            );
            let value = serde_json::to_string(&text).map_err(|e| e.to_string())?;
            Some(vec!["-c".into(), format!("developer_instructions={value}")])
        }
        Provider::Cursor => None,
    })
}

/// Prompt arguments for a resumed launch whose callback folder holds a prompt.
pub(crate) fn resume_prompt_args(provider: Provider, spool: &Path) -> Result<Vec<String>, String> {
    let path = spool.join(PROMPT_FILE);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    // A launch that adopted a session resumes the same way.
    let adopted = super::launch::reserved_session(spool).is_some();
    Ok(prompt_args(provider, &path, adopted)?.unwrap_or_default())
}

/// The first message when the provider cannot take the prompt at launch.
pub(crate) fn prompt_message(text: &str) -> String {
    format!(
        "Bus: this is your system prompt; Bus could not set it at launch for this provider or session. Follow it for the rest of this session, then reply briefly that you are ready.\n\n{text}"
    )
}

/// Tells an orchestrator its new room: its system prompt is fixed at launch.
pub(crate) fn reassigned_message(agent: &str, room: Option<(&str, RoomId)>) -> String {
    match room {
        Some((name, id)) => format!(
            "Bus: you now orchestrate room {name} (id {id}). This replaces the room in your system prompt. Send to it with `bus send --room {id} --as {agent} ...`; start with `bus state` and `bus history --room {id}`.",
            id = id.0
        ),
        None => "Bus: you no longer orchestrate a room. Wait for the human to assign one.".into(),
    }
}

#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod tests;
