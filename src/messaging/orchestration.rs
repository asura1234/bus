//! A MASTER orchestrator's system prompt and the Bus docs it references. The
//! prompt is a Bus-owned file in the launch's callback folder, delivered with
//! each provider's own launch option; nothing is written into the agent's PWD.
use std::path::{Path, PathBuf};

use super::model::{Provider, RoomId};
use super::storage::io::{atomic_write, private_dir};

/// The default orchestrator system prompt, with `{{...}}` placeholders.
pub(crate) const DEFAULT_PROMPT: &str = include_str!("../../orchestration/prompt.md");
const WORKFLOW_CREATE: &str = include_str!("../../workflows/create.md");
/// The docs the prompt references, written under `<BUS_DATA_DIR>/docs/`.
const DOCS: &[(&str, &str)] = &[
    (
        "how-to-bus-cli.md",
        include_str!("../../orchestration/how-to-bus-cli.md"),
    ),
    (
        "orchestrator-guide.md",
        include_str!("../../orchestration/guide.md"),
    ),
    (
        "orchestrator-rules.md",
        include_str!("../../orchestration/rules.md"),
    ),
    (
        "templates/workflow-template.md",
        include_str!("../../workflows/template.md"),
    ),
    (
        "workflows/pr-review-loop.md",
        include_str!("../../workflows/pr-review-loop.md"),
    ),
    (
        "workflows/cross-repo-feature.md",
        include_str!("../../workflows/cross-repo-feature.md"),
    ),
];
/// The prompt file in a launch's callback folder; resumes deliver it again.
pub(crate) const PROMPT_FILE: &str = "system-prompt.md";
// Only the add-agent form's preview shows these: a launch always names a room.
const UNASSIGNED_ROOM_NAME: &str = "none yet (choose a work room)";
const UNASSIGNED_ROOM_ID: &str = "ROOM";

/// What a MASTER agent is created with, beyond the plain agent input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OrchestratorSpec {
    /// The work room it orchestrates for its whole life; required, because
    /// the system prompt names it at launch and is never re-sent.
    pub room: RoomId,
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

/// Writes the embedded Bus docs under `<data_dir>/docs/` and returns that folder.
pub(crate) fn write_docs(data_dir: &Path) -> Result<PathBuf, String> {
    let root = docs_dir(data_dir);
    let docs = DOCS
        .iter()
        .copied()
        .chain([("workflow-create.md", WORKFLOW_CREATE)]);
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

/// Why Codex cannot be added as an orchestrator.
pub(crate) const CODEX_ORCHESTRATOR_REFUSED: &str = "Codex cannot be an orchestrator yet: an orchestrator dispatches work with bus send --async in the background and must be woken when it finishes, but Codex does not start a new turn when a background command exits (openai/codex issues 32188, 50079). Use Claude Code or Cursor as the orchestrator; Codex works fine as a worker.";

/// Checks the provider of a new orchestrator. An orchestrator waits on its
/// workers through background `bus send --async` commands and relies on the
/// provider starting a new turn when one exits; Codex does not, so it would
/// never wake. Only new adds come here: Codex orchestrators already in a saved
/// session keep loading and resuming, and Codex workers are unaffected.
pub(crate) fn check_new_orchestrator(provider: Provider) -> Result<(), String> {
    if provider == Provider::Codex {
        return Err(CODEX_ORCHESTRATOR_REFUSED.into());
    }
    Ok(())
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
        // with and ignores new ones, so an adopted thread gets a message. New
        // Codex orchestrators are refused (`check_new_orchestrator`); this
        // remains for ones saved sessions already hold.
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
    let adopted = super::provider_glue::launch::reserved_session(spool).is_some();
    Ok(prompt_args(provider, &path, adopted)?.unwrap_or_default())
}

/// The first message when the provider cannot take the prompt at launch.
pub(crate) fn prompt_message(text: &str) -> String {
    format!(
        "Bus: this is your system prompt; Bus could not set it at launch for this provider or session. Follow it for the rest of this session, then reply briefly that you are ready.\n\n{text}"
    )
}

#[cfg(test)]
#[path = "tests/orchestration_test.rs"]
mod tests;
