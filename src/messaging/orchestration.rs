//! A MASTER orchestrator's system prompt and the Bus docs it references. The
//! prompt is a Bus-owned file in the launch's callback folder, delivered with
//! each provider's own launch option; nothing is written into the agent's PWD.
use std::path::{Path, PathBuf};

use super::model::{Provider, RoomId};
use super::storage::io::{atomic_write, private_dir};

/// The default orchestrator system prompt, with `{{...}}` placeholders.
pub(crate) const DEFAULT_PROMPT: &str = include_str!("../../orchestration/prompt.md");
// An authored default with an unknown or unfinished placeholder cannot ship.
const _: () = assert!(supported_prompt_placeholders(DEFAULT_PROMPT));
const WORKFLOW_CREATE: &str = include_str!("../../workflows/create.md");
/// The docs the prompt references, written under `<BUS_DATA_DIR>/docs/`.
const DOCS: &[(&str, &str)] = &[
    (
        "providers/grok.md",
        include_str!("../../orchestration/providers/grok.md"),
    ),
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
        "templates/investigation-report.md",
        include_str!("../../workflows/investigation-report-template.md"),
    ),
    (
        "templates/bug-triage-table.md",
        include_str!("../../workflows/bug-triage-table-template.md"),
    ),
];
/// Original filenames are the public lookup contract for developer-named workflows.
const WORKFLOWS: &[(&str, &str)] = &[
    ("create.md", WORKFLOW_CREATE),
    ("template.md", include_str!("../../workflows/template.md")),
    (
        "investigation-report-template.md",
        include_str!("../../workflows/investigation-report-template.md"),
    ),
    (
        "bug-triage-table-template.md",
        include_str!("../../workflows/bug-triage-table-template.md"),
    ),
    (
        "agent-handover.md",
        include_str!("../../workflows/agent-handover.md"),
    ),
    (
        "handover-template.md",
        include_str!("../../workflows/handover-template.md"),
    ),
    (
        "auto-merge-pr.md",
        include_str!("../../workflows/auto-merge-pr.md"),
    ),
    ("bug-fix.md", include_str!("../../workflows/bug-fix.md")),
    (
        "cross-repo-feature.md",
        include_str!("../../workflows/cross-repo-feature.md"),
    ),
];
/// The prompt file in a launch's callback folder; resumes deliver it again.
pub(crate) const PROMPT_FILE: &str = "system-prompt.md";
// Only the add-agent form's preview shows these: a launch always names a room.
const UNASSIGNED_ROOM_NAME: &str = "none yet (choose a work room)";
const UNASSIGNED_ROOM_ID: &str = "ROOM";

const fn supported_prompt_placeholders(template: &str) -> bool {
    let placeholders = [
        "{{ROOM_NAME}}",
        "{{ROOM_ID}}",
        "{{AGENT_NAME}}",
        "{{DOCS}}",
        "{{WORKFLOWS_DIR}}",
    ];
    let bytes = template.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] != b'{' || bytes[index + 1] != b'{' {
            index += 1;
            continue;
        }
        let mut candidate = 0;
        let mut matched = false;
        while candidate < placeholders.len() {
            let token = placeholders[candidate].as_bytes();
            let mut offset = 0;
            while offset < token.len()
                && index + offset < bytes.len()
                && bytes[index + offset] == token[offset]
            {
                offset += 1;
            }
            if offset == token.len() {
                matched = true;
                index += offset;
                break;
            }
            candidate += 1;
        }
        if !matched {
            return false;
        }
    }
    true
}

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

/// One resolver for the launch prompt, form preview and binding rules.
fn workflows_dir(docs: &Path) -> PathBuf {
    workflows_dir_at(
        docs,
        cfg!(debug_assertions) || super::diagnostics::dev_enabled(),
        std::env::current_exe().ok().as_deref(),
        std::env::current_dir().ok().as_deref(),
    )
}

fn workflows_dir_at(
    docs: &Path,
    dev: bool,
    executable: Option<&Path>,
    cwd: Option<&Path>,
) -> PathBuf {
    if dev {
        for anchor in [executable.and_then(Path::parent), cwd]
            .into_iter()
            .flatten()
        {
            if let Some(repo) = crate::utils::paths::find_bus_checkout(anchor) {
                let workflows = repo.join("workflows");
                if workflows.is_dir() {
                    return workflows;
                }
            }
        }
    }
    docs.join("workflows")
}

pub(crate) fn fill(template: &str, values: &PromptValues) -> String {
    let (room_name, room_id) = match &values.room {
        Some((name, id)) => (name.clone(), id.0.to_string()),
        None => (UNASSIGNED_ROOM_NAME.into(), UNASSIGNED_ROOM_ID.into()),
    };
    let docs = values.docs.to_string_lossy();
    let workflows = workflows_dir(&values.docs);
    let workflows = workflows.to_string_lossy();
    let tokens = [
        ("{{ROOM_NAME}}", room_name.as_str()),
        ("{{ROOM_ID}}", room_id.as_str()),
        ("{{AGENT_NAME}}", values.agent.as_str()),
        ("{{DOCS}}", &docs),
        ("{{WORKFLOWS_DIR}}", &workflows),
    ];
    // One pass over the template: inserted values are never scanned again, so a
    // room or agent name containing placeholder text stays literal. On a `{`
    // that starts no token, only that brace is copied, so `{{{ROOM_ID}}}` still
    // finds the token at the next brace.
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match tokens.iter().find(|(token, _)| rest.starts_with(token)) {
            Some((token, value)) => {
                out.push_str(value);
                rest = &rest[token.len()..];
            }
            None => {
                out.push('{');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Writes the embedded Bus docs under `<data_dir>/docs/` and returns that folder.
pub(crate) fn write_docs(data_dir: &Path) -> Result<PathBuf, String> {
    let root = docs_dir(data_dir);
    let workflows = workflows_dir(&root);
    let docs = DOCS
        .iter()
        .copied()
        .chain([("workflow-create.md", WORKFLOW_CREATE)]);
    for (name, text) in docs {
        if name == "orchestrator-rules.md" {
            let rules = text.replace("{{WORKFLOWS_DIR}}", &workflows.to_string_lossy());
            write_doc(&root, name, &rules)?;
        } else {
            // Placeholder examples in the CLI guide are documentation, not inputs.
            write_doc(&root, name, text)?;
        }
    }
    for (name, text) in WORKFLOWS {
        write_doc(&root.join("workflows"), name, text)?;
    }
    Ok(root)
}

fn write_doc(root: &Path, name: &str, text: &str) -> Result<(), String> {
    let path = root.join(name);
    private_dir(path.parent().expect("doc has a parent")).map_err(|e| e.to_string())?;
    atomic_write(&path, text.as_bytes()).map_err(|e| e.to_string())
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
        Provider::Grok => {
            crate::agents::providers::grok::system_prompt::prompt_args(path, adopted)?
        }
        Provider::ClaudeCode => {
            Some(crate::agents::providers::claude_code::system_prompt::prompt_args(path, adopted))
        }
        Provider::Codex => {
            crate::agents::providers::codex::system_prompt::prompt_args(path, adopted)?
        }
        Provider::Cursor => {
            crate::agents::providers::cursor::system_prompt::prompt_args(path, adopted)?
        }
    })
}

/// Prompt arguments for a resumed launch whose callback folder holds a prompt.
pub(crate) fn resume_prompt_args(provider: Provider, spool: &Path) -> Result<Vec<String>, String> {
    let path = spool.join(PROMPT_FILE);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    // A launch that adopted a session resumes the same way.
    let adopted = crate::agents::providers::launch::reserved_session(spool).is_some();
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

#[cfg(test)]
#[path = "tests/grok_system_prompt_test.rs"]
mod grok_tests;
