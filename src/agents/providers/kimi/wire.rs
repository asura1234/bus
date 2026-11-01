//! Kimi 2.4.0's private wire is the completion witness. Stop is only a wake.
use serde_json::Value;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};

const MAX_WIRE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    Final(String),
    NoReply,
    Failed(String),
}

/// One Kimi session read for the prompt Bus bound, plus steered submissions
/// and a later non-Bus continuation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionCapture {
    /// The original prompt's outcome. A final that belongs to a mid-turn steer
    /// is absent here, not reused as this prompt's answer.
    pub lead: Option<Outcome>,
    /// `(typed prompt, assistant text)` for a steer inside the bound turn, or
    /// for a later Bus turn that carried that typed prompt on its own.
    pub steered: Vec<(String, String)>,
    /// `(turn id, assistant text)` when a later turn, not a Bus message, is
    /// the original prompt's own answer.
    pub continuation: Option<(u64, String)>,
}

#[cfg(test)]
pub(crate) fn committed_final(wire: &str, turn_id: u64, prompt: &str) -> Option<String> {
    match committed_outcome(wire, turn_id, prompt)? {
        Outcome::Final(text) => Some(text),
        Outcome::NoReply | Outcome::Failed(_) => None,
    }
}

#[cfg(test)]
pub(crate) fn committed_outcome(wire: &str, turn_id: u64, prompt: &str) -> Option<Outcome> {
    committed_outcome_lines(
        wire.lines().map(|line| Ok(line.to_owned())),
        turn_id,
        prompt,
    )
    .ok()
    .flatten()
}

fn normalized_prompt(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches(|character: char| character.is_ascii_whitespace())
        .to_owned()
}

#[cfg(test)]
fn committed_outcome_lines(
    lines: impl IntoIterator<Item = Result<String, String>>,
    turn_id: u64,
    prompt: &str,
) -> Result<Option<Outcome>, String> {
    let mut lines = lines.into_iter();
    let mut invalid = None;
    let checked = std::iter::from_fn(|| {
        let line = lines.next()?;
        match line {
            Err(error) => {
                invalid = Some(error);
                None
            }
            Ok(line) if line.len() as u64 > MAX_WIRE_BYTES => Some(Ok(line)),
            Ok(line) => match serde_json::from_str::<Value>(&line) {
                Ok(_) => Some(Ok(line)),
                Err(_) => {
                    invalid = Some("Kimi wire contains malformed JSON".into());
                    None
                }
            },
        }
    });
    let outcome =
        session_capture_lines_inner(checked, turn_id, prompt).and_then(|capture| capture.lead);
    if let Some(error) = invalid {
        return Err(error);
    }
    Ok(outcome)
}

pub(crate) fn session_capture_lines(
    lines: impl IntoIterator<Item = Result<String, String>>,
    turn_id: u64,
    prompt: &str,
) -> Result<Option<SessionCapture>, String> {
    let mut lines = lines.into_iter();
    let mut invalid = None;
    let checked = std::iter::from_fn(|| {
        let line = lines.next()?;
        match line {
            Err(error) => {
                invalid = Some(error);
                None
            }
            Ok(line) if line.len() as u64 > MAX_WIRE_BYTES => Some(Ok(line)),
            Ok(line) => match serde_json::from_str::<Value>(&line) {
                Ok(_) => Some(Ok(line)),
                Err(_) => {
                    invalid = Some("Kimi wire contains malformed JSON".into());
                    None
                }
            },
        }
    });
    let capture = session_capture_lines_inner(checked, turn_id, prompt);
    if let Some(error) = invalid {
        return Err(error);
    }
    Ok(capture)
}

struct OpenTurn {
    id: u64,
    prompt_text: String,
    prompt_id: String,
    lead_answer: Option<String>,
    after_steer: bool,
    steer_prompt: Option<String>,
    steer_answer: Option<String>,
    steers: Vec<(String, String)>,
    /// The lead called a tool and has not written its own answer since.
    saw_lead_tool: bool,
    agent_outcome: Option<String>,
    agent_error: Option<String>,
    turn_reason: Option<String>,
    prompt_reason: Option<String>,
}

fn assistant_text(value: &Value) -> Option<(bool, String)> {
    let message = value.get("message")?.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("assistant")
        || value
            .pointer("/message/meta/source")
            .and_then(Value::as_str)
            != Some("llm")
    {
        return None;
    }
    let tool_call = message
        .get("toolCalls")
        .and_then(Value::as_array)
        .is_some_and(|calls| !calls.is_empty());
    if tool_call {
        return Some((true, String::new()));
    }
    let text = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Option<Vec<_>>>()?
        .concat();
    Some((false, text))
}

fn steer_text(value: &Value) -> Option<String> {
    let parts = value
        .get("input")
        .or_else(|| value.get("content"))?
        .as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

fn note_steer(turn: &mut OpenTurn, text: String) {
    if turn.steer_prompt.as_ref() == Some(&text) {
        turn.after_steer = true;
        return;
    }
    if let (Some(prompt), Some(answer)) = (turn.steer_prompt.take(), turn.steer_answer.take()) {
        if !answer.is_empty() {
            turn.steers.push((prompt, answer));
        }
    }
    turn.steer_prompt = Some(text);
    turn.steer_answer = None;
    turn.after_steer = true;
}

fn finish_turn(turn: &mut OpenTurn) {
    if let (Some(prompt), Some(answer)) = (turn.steer_prompt.take(), turn.steer_answer.take()) {
        if !answer.is_empty() {
            turn.steers.push((prompt, answer));
        }
    }
}

fn continuation_text(turn: &OpenTurn) -> Option<String> {
    if turn.prompt_text.contains("[Bus message") {
        return None;
    }
    if turn.agent_outcome.as_deref() == Some("done")
        && turn.turn_reason.as_deref() == Some("completed")
    {
        turn.lead_answer.clone().filter(|text| !text.is_empty())
    } else {
        None
    }
}

fn open_turn(value: &Value) -> Option<OpenTurn> {
    if value.get("agentId").and_then(Value::as_str) != Some("main") {
        return None;
    }
    let id = value.get("turnId").and_then(Value::as_u64)?;
    let input = value.get("input").and_then(Value::as_array)?;
    if input.len() != 1 {
        return None;
    }
    let text = input
        .first()
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)?;
    Some(OpenTurn {
        id,
        prompt_text: text.to_owned(),
        prompt_id: value
            .get("promptId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        lead_answer: None,
        after_steer: false,
        steer_prompt: None,
        steer_answer: None,
        steers: Vec::new(),
        saw_lead_tool: false,
        agent_outcome: None,
        agent_error: None,
        turn_reason: None,
        prompt_reason: None,
    })
}

fn note_assistant(turn: &mut OpenTurn, value: &Value) {
    let Some((tool_call, text)) = assistant_text(value) else {
        return;
    };
    if tool_call {
        if turn.after_steer {
            turn.steer_answer = None;
        } else {
            turn.lead_answer = None;
            turn.saw_lead_tool = true;
        }
    } else if !text.is_empty() {
        if turn.after_steer {
            turn.steer_answer = Some(text);
        } else {
            turn.lead_answer = Some(text);
        }
    }
}

fn apply_wire_event(turn: &mut OpenTurn, kind: &str, value: &Value) {
    match kind {
        "prompt.steered" | "turn.steer" => {
            if value
                .get("turnId")
                .and_then(Value::as_u64)
                .is_some_and(|id| id != turn.id)
            {
                return;
            }
            if let Some(text) = steer_text(value) {
                note_steer(turn, text);
            }
        }
        "agent.message.appended" if turn.agent_outcome.is_none() => note_assistant(turn, value),
        "agent.turn.ended" if value.get("turnId").and_then(Value::as_u64) == Some(turn.id) => {
            if let Some(outcome) = value.get("outcome").and_then(Value::as_str) {
                turn.agent_outcome = Some(outcome.to_owned());
                turn.agent_error = value
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
        }
        "turn.ended"
            if value.get("agentId").and_then(Value::as_str) == Some("main")
                && value.get("turnId").and_then(Value::as_u64) == Some(turn.id)
                && turn.agent_outcome.is_some() =>
        {
            if let Some(reason) = value.get("reason").and_then(Value::as_str) {
                turn.turn_reason = Some(reason.to_owned());
            }
        }
        "prompt.completed"
            if value.get("agentId").and_then(Value::as_str) == Some("main")
                && value.get("promptId").and_then(Value::as_str) == Some(&turn.prompt_id) =>
        {
            if let Some(reason) = value.get("reason").and_then(Value::as_str) {
                turn.prompt_reason = Some(reason.to_owned());
            }
        }
        _ => {}
    }
}

fn prompt_matches(turn: &OpenTurn, prompt: &str) -> bool {
    normalized_prompt(&turn.prompt_text) == normalized_prompt(prompt)
}

fn find_bound<'a>(done: &'a [OpenTurn], turn_id: u64, prompt: &str) -> Option<&'a OpenTurn> {
    if let Some(exact) = done
        .iter()
        .find(|turn| turn.id == turn_id && prompt_matches(turn, prompt))
    {
        return Some(exact);
    }
    // A missing turn id is still in progress. Only a stored id that now points
    // at a different Bus message may fall back to the earlier prompt turn.
    let stored = done.iter().find(|turn| turn.id == turn_id)?;
    if !stored.prompt_text.contains("[Bus message") {
        return None;
    }
    done.iter()
        .find(|turn| turn.id < turn_id && prompt_matches(turn, prompt))
}

fn empty_capture() -> SessionCapture {
    SessionCapture {
        lead: None,
        steered: Vec::new(),
        continuation: None,
    }
}

fn own_turn_final(turn: &OpenTurn) -> Option<String> {
    if turn.prompt_reason.as_deref() != Some("completed")
        || turn.agent_outcome.as_deref() != Some("done")
        || turn.turn_reason.as_deref() != Some("completed")
    {
        return None;
    }
    turn.lead_answer.clone().filter(|text| !text.is_empty())
}

/// A later `turn.prompt` that is itself a Bus message belongs to that message,
/// not to the original prompt. An open one means its answer is not committed.
fn later_bus_answer(turn: &OpenTurn, lead_prompt: &str) -> Option<(String, String)> {
    if prompt_matches(turn, lead_prompt) || !turn.prompt_text.contains("[Bus message") {
        return None;
    }
    own_turn_final(turn).map(|text| (turn.prompt_text.clone(), text))
}

/// Only a Bus turn that started after the bound turn can withhold it. An
/// earlier turn that never got `prompt.completed` is a different request.
fn later_bus_open(turn: &OpenTurn, bound: &OpenTurn) -> bool {
    turn.id > bound.id
        && !prompt_matches(turn, &bound.prompt_text)
        && turn.prompt_text.contains("[Bus message")
        && turn.prompt_reason.is_none()
}

fn push_steered(steered: &mut Vec<(String, String)>, pair: (String, String)) {
    if !steered.iter().any(|(existing, _)| existing == &pair.0) {
        steered.push(pair);
    }
}

/// The follow-up turn's own final, plus any steer recorded inside that turn.
fn later_turn_replies(turn: &OpenTurn, lead_prompt: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if let Some(pair) = later_bus_answer(turn, lead_prompt) {
        pairs.push(pair);
    }
    if prompt_matches(turn, lead_prompt) || !turn.prompt_text.contains("[Bus message") {
        return pairs;
    }
    pairs.extend(turn.steers.iter().cloned());
    pairs
}

fn lead_after_steers(
    bound: &OpenTurn,
    done: &[OpenTurn],
    lead_outcome: Outcome,
    steered: &[(String, String)],
) -> (Option<Outcome>, Option<(u64, String)>) {
    let Outcome::NoReply = lead_outcome else {
        return (Some(lead_outcome), None);
    };
    if steered.is_empty() {
        return (Some(Outcome::NoReply), None);
    }
    let continuation = done
        .iter()
        .filter(|turn| turn.id > bound.id)
        .find_map(|turn| continuation_text(turn).map(|text| (turn.id, text)));
    if let Some(continuation) = continuation {
        return (None, Some(continuation));
    }
    // A tool with no lead text can still finish on a later non-Bus turn.
    // A steer reply alone is that steer's answer, not proof the lead is open.
    if bound.saw_lead_tool && bound.lead_answer.is_none() {
        (None, None)
    } else {
        (Some(Outcome::NoReply), None)
    }
}

fn capture_bound(done: &[OpenTurn], turn_id: u64, prompt: &str) -> Option<SessionCapture> {
    let bound = find_bound(done, turn_id, prompt)?;
    // prompt.completed is the bound turn's commit. A steer answer or a later
    // continuation is not evidence until that chain is a recognized terminal.
    if bound.prompt_reason.is_none() {
        return Some(empty_capture());
    }
    let Some(lead_outcome) = terminal_outcome(
        bound.agent_outcome.as_deref(),
        bound.turn_reason.as_deref(),
        bound.prompt_reason.as_deref().unwrap_or(""),
        bound.lead_answer.clone(),
        bound.agent_error.clone(),
    ) else {
        return Some(empty_capture());
    };
    let mut steered = bound.steers.clone();
    // The follow-up started a turn of its own. Hold the original prompt until
    // that turn commits, then give the follow-up its own text.
    if done.iter().any(|turn| later_bus_open(turn, bound)) {
        return Some(SessionCapture {
            lead: None,
            steered,
            continuation: None,
        });
    }
    for turn in done.iter().filter(|turn| turn.id > bound.id) {
        for pair in later_turn_replies(turn, prompt) {
            push_steered(&mut steered, pair);
        }
    }
    let (lead, continuation) = lead_after_steers(bound, done, lead_outcome, &steered);
    Some(SessionCapture {
        lead,
        steered,
        continuation,
    })
}

fn session_capture_lines_inner(
    lines: impl IntoIterator<Item = Result<String, String>>,
    turn_id: u64,
    prompt: &str,
) -> Option<SessionCapture> {
    let mut open: Option<OpenTurn> = None;
    let mut done: Vec<OpenTurn> = Vec::new();
    for line in lines {
        let Ok(line) = line else {
            continue;
        };
        if line.len() as u64 > MAX_WIRE_BYTES {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        if kind == "turn.prompt" {
            if let Some(mut previous) = open.take() {
                finish_turn(&mut previous);
                done.push(previous);
            }
            open = open_turn(&value);
            continue;
        }
        if let Some(turn) = open.as_mut() {
            apply_wire_event(turn, kind, &value);
        }
    }
    if let Some(mut previous) = open.take() {
        finish_turn(&mut previous);
        done.push(previous);
    }
    capture_bound(&done, turn_id, prompt)
}

fn terminal_outcome(
    agent: Option<&str>,
    turn: Option<&str>,
    prompt: &str,
    answer: Option<String>,
    error: Option<String>,
) -> Option<Outcome> {
    match (agent, turn, prompt) {
        (Some("done"), Some("completed"), "completed") => {
            Some(answer.map_or(Outcome::NoReply, Outcome::Final))
        }
        (Some("failed"), Some("failed"), "failed") => Some(Outcome::Failed(
            error.unwrap_or_else(|| "Kimi turn failed".into()),
        )),
        (Some("interrupted"), Some("interrupted"), "interrupted") => {
            Some(Outcome::Failed("Kimi turn interrupted".into()))
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn resolve(
    home: &Path,
    cwd: &Path,
    session: &str,
    turn: &str,
    prompt: &str,
) -> Result<Option<String>, String> {
    Ok(match resolve_outcome(home, cwd, session, turn, prompt)? {
        Some(Outcome::Final(text)) => Some(text),
        _ => None,
    })
}

#[cfg(test)]
pub(crate) fn resolve_outcome(
    home: &Path,
    cwd: &Path,
    session: &str,
    turn: &str,
    prompt: &str,
) -> Result<Option<Outcome>, String> {
    Ok(resolve_capture(home, cwd, session, turn, prompt)?.and_then(|capture| capture.lead))
}

pub(crate) fn resolve_capture(
    home: &Path,
    cwd: &Path,
    session: &str,
    turn: &str,
    prompt: &str,
) -> Result<Option<SessionCapture>, String> {
    let turn_id = turn.parse::<u64>().map_err(|_| "Invalid Kimi turn id")?;
    let root = home.canonicalize().map_err(|e| e.to_string())?;
    let index = home.join("session_index.jsonl");
    if std::fs::symlink_metadata(&index).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("Kimi session index is a symlink".into());
    }
    let bytes = match std::fs::metadata(&index) {
        Ok(meta) if meta.len() <= MAX_INDEX_BYTES => {
            std::fs::read(&index).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err("Kimi session index is oversized".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let index = String::from_utf8(bytes).map_err(|_| "Kimi session index is not UTF-8")?;
    let mut directory: Option<PathBuf> = None;
    for line in index.lines() {
        let record: Value =
            serde_json::from_str(line).map_err(|_| "Kimi session index is incomplete")?;
        if record.get("sessionId").and_then(Value::as_str) == Some(session) {
            if record.get("workDir").and_then(Value::as_str) != cwd.to_str() {
                return Err("Kimi session cwd differs from its Bus owner".into());
            }
            let path = PathBuf::from(
                record
                    .get("sessionDir")
                    .and_then(Value::as_str)
                    .ok_or("Kimi session has no directory")?,
            );
            if !path.is_absolute() {
                return Err("Kimi session directory is not absolute".into());
            }
            directory = Some(path);
        }
    }
    let Some(directory) = directory else {
        return Ok(None);
    };
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    if !directory.starts_with(&root)
        || directory.file_name().and_then(|name| name.to_str()) != Some(session)
    {
        return Err("Kimi session directory escaped its private home".into());
    }
    let wire = directory.join("agents/main/wire.jsonl");
    if std::fs::symlink_metadata(&wire).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("Kimi wire is a symlink".into());
    }
    let file = match std::fs::File::open(&wire) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let lines = std::io::BufReader::new(file)
        .lines()
        .map(|line| line.map_err(|error| error.to_string()));
    session_capture_lines(lines, turn_id, prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_outcome_requires_the_owned_prompt_and_complete_consistent_chain() {
        let start = "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":7,\"promptId\":\"p7\",\"input\":[{\"type\":\"text\",\"text\":\"Bus task\"}]}\n";
        let agent = "{\"type\":\"agent.turn.ended\",\"turnId\":7,\"outcome\":\"failed\"}\n";
        let turn =
            "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":7,\"reason\":\"failed\"}\n";
        let prompt = "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p7\",\"reason\":\"failed\"}\n";
        assert_eq!(committed_outcome(start, 7, "Bus task"), None);
        assert_eq!(
            committed_outcome(&format!("{start}{agent}{turn}"), 7, "Bus task"),
            None
        );
        let finished = format!("{start}{agent}{turn}{prompt}");
        assert_eq!(
            committed_outcome(&finished, 7, "Bus task"),
            Some(Outcome::Failed("Kimi turn failed".into()))
        );
        let with_error = finished.replace(
            "\"outcome\":\"failed\"",
            "\"outcome\":\"failed\",\"errorMessage\":\"OAuthUnauthorizedError: login required\"",
        );
        assert_eq!(
            committed_outcome(&with_error, 7, "Bus task"),
            Some(Outcome::Failed(
                "OAuthUnauthorizedError: login required".into()
            ))
        );
        assert_eq!(committed_outcome(&finished, 7, "other"), None);
        assert_eq!(
            committed_outcome(
                &finished.replace("\"reason\":\"failed\"", "\"reason\":\"completed\""),
                7,
                "Bus task"
            ),
            None
        );
        let success = finished
            .replace("\"outcome\":\"failed\"", "\"outcome\":\"done\"")
            .replace("\"reason\":\"failed\"", "\"reason\":\"completed\"");
        assert_eq!(
            committed_outcome(&success, 7, "Bus task"),
            Some(Outcome::NoReply)
        );
    }

    #[test]
    fn resolver_requires_its_private_session_cwd_and_committed_turn() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-wire-resolve");
        let cwd = root.join("project");
        let session = "session_test";
        let directory = root.join("sessions/wd_project/session_test");
        crate::platform::fs::private_dir(&cwd).unwrap();
        crate::platform::fs::private_dir(&directory.join("agents/main")).unwrap();
        let index = serde_json::json!({
            "sessionId": session,
            "workDir": cwd,
            "sessionDir": directory,
        });
        std::fs::write(root.join("session_index.jsonl"), format!("{index}\n")).unwrap();
        let wire = concat!(
            "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":7,\"promptId\":\"p7\",\"input\":[{\"type\":\"text\",\"text\":\"Bus task\"}]}\n",
            "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
            "{\"type\":\"agent.turn.ended\",\"turnId\":7,\"outcome\":\"done\"}\n",
            "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":7,\"reason\":\"completed\"}\n",
            "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p7\",\"reason\":\"completed\"}\n",
        );
        std::fs::write(directory.join("agents/main/wire.jsonl"), wire).unwrap();
        assert_eq!(
            resolve(&root, &cwd, session, "7", "Bus task").unwrap(),
            Some("Done".into())
        );
        assert_eq!(
            resolve(&root, &cwd, session, "7", "Other task").unwrap(),
            None
        );
        assert_eq!(
            resolve(&root, &cwd, session, "8", "Bus task").unwrap(),
            None
        );
        assert!(resolve(&root, &root, session, "7", "Bus task").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_final_uses_the_start_hook_trailing_whitespace_contract() {
        let wire = concat!(
            "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":7,\"promptId\":\"p7\",\"input\":[{\"type\":\"text\",\"text\":\"Bus task\"}]}\n",
            "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
            "{\"type\":\"agent.turn.ended\",\"turnId\":7,\"outcome\":\"done\"}\n",
            "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":7,\"reason\":\"completed\"}\n",
            "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p7\",\"reason\":\"completed\"}\n",
        );
        assert_eq!(
            committed_outcome(wire, 7, "Bus task\n"),
            Some(Outcome::Final("Done".into())),
            "the start hook and committed wire must accept the same trailing newline normalization"
        );
    }

    #[test]
    fn committed_final_is_found_after_benign_history_exceeds_storage_cap() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-wire-long-history");
        let cwd = root.join("project");
        let session = "session_long";
        let directory = root.join("sessions/wd_project/session_long");
        crate::platform::fs::private_dir(&cwd).unwrap();
        crate::platform::fs::private_dir(&directory.join("agents/main")).unwrap();
        let index = serde_json::json!({
            "sessionId": session,
            "workDir": cwd,
            "sessionDir": directory,
        });
        std::fs::write(root.join("session_index.jsonl"), format!("{index}\n")).unwrap();
        let mut wire = concat!(
            "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":7,\"promptId\":\"p7\",\"input\":[{\"type\":\"text\",\"text\":\"Bus task\"}]}\n",
            "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
            "{\"type\":\"agent.turn.ended\",\"turnId\":7,\"outcome\":\"done\"}\n",
            "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":7,\"reason\":\"completed\"}\n",
            "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p7\",\"reason\":\"completed\"}\n",
        )
        .to_owned();
        wire.push_str(&" ".repeat(MAX_WIRE_BYTES as usize + 1));
        std::fs::write(directory.join("agents/main/wire.jsonl"), wire).unwrap();
        assert_eq!(
            resolve(&root, &cwd, session, "7", "Bus task").unwrap(),
            Some("Done".into()),
            "bytes after this turn must not disable its committed reply"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_final_survives_an_oversized_line_before_the_turn() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-wire-old-history");
        let cwd = root.join("project");
        let session = "session_old";
        let directory = root.join("sessions/wd_project/session_old");
        crate::platform::fs::private_dir(&cwd).unwrap();
        crate::platform::fs::private_dir(&directory.join("agents/main")).unwrap();
        let index = serde_json::json!({
            "sessionId": session,
            "workDir": cwd,
            "sessionDir": directory,
        });
        std::fs::write(root.join("session_index.jsonl"), format!("{index}\n")).unwrap();
        let mut wire = " ".repeat(MAX_WIRE_BYTES as usize + 1);
        wire.push('\n');
        wire.push_str(concat!(
            "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":7,\"promptId\":\"p7\",\"input\":[{\"type\":\"text\",\"text\":\"Bus task\"}]}\n",
            "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
            "{\"type\":\"agent.turn.ended\",\"turnId\":7,\"outcome\":\"done\"}\n",
            "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":7,\"reason\":\"completed\"}\n",
            "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p7\",\"reason\":\"completed\"}\n",
        ));
        std::fs::write(directory.join("agents/main/wire.jsonl"), wire).unwrap();
        assert_eq!(
            resolve(&root, &cwd, session, "7", "Bus task").unwrap(),
            Some("Done".into()),
            "older history must not disable this committed turn's reply"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn kimi_steered_reply_is_not_the_lead_final() {
        let original = "[Bus message 5 from you]\nRun sleep 45 and reply KIMI-BUSY-DONE-32299.";
        let steered = "[Bus message 8 from you]\nReply with exactly KIMI-QUEUED-32299.";
        let wire = format!(
            concat!(
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":0,\"promptId\":\"p0\",\"input\":[{{\"type\":\"text\",\"text\":{original}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"working\"}}],\"toolCalls\":[{{\"name\":\"bash\"}}]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"prompt.steered\",\"agentId\":\"main\",\"content\":[{{\"type\":\"text\",\"text\":{steered}}}]}}\n",
                "{{\"type\":\"turn.steer\",\"agentId\":\"main\",\"turnId\":0,\"input\":[{{\"type\":\"text\",\"text\":{steered}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"KIMI-QUEUED-32299\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":0,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":0,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p0\",\"reason\":\"completed\"}}\n",
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":1,\"promptId\":\"p1\",\"input\":[{{\"type\":\"text\",\"text\":\"<notification id=\\\"task:done\\\" />\"}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"KIMI-BUSY-DONE-32299\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":1,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":1,\"reason\":\"completed\"}}\n",
            ),
            original = serde_json::to_string(original).unwrap(),
            steered = serde_json::to_string(steered).unwrap(),
        );
        assert_eq!(
            committed_outcome(&wire, 0, original),
            None,
            "the steered assistant text is not the original prompt's final"
        );
        let capture =
            session_capture_lines(wire.lines().map(|line| Ok(line.to_owned())), 0, original)
                .unwrap()
                .expect("bound turn completed");
        assert_eq!(
            capture.steered,
            vec![(steered.to_owned(), "KIMI-QUEUED-32299".to_owned())]
        );
        assert_eq!(
            capture.continuation,
            Some((1, "KIMI-BUSY-DONE-32299".to_owned()))
        );
    }

    #[test]
    fn kimi_follow_up_on_its_own_turn_is_not_the_lead_reply() {
        let original = "[Bus message 5 from you]\nRun sleep 45 and reply KIMI-BUSY-DONE-32299.";
        let follow = "[Bus message 8 from you]\nReply with exactly KIMI-QUEUED-32299.";
        let wire = format!(
            concat!(
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":0,\"promptId\":\"p0\",\"input\":[{{\"type\":\"text\",\"text\":{original}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"\"}}],\"toolCalls\":[{{\"name\":\"bash\"}}]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"KIMI-BUSY-DONE-32299\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":0,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":0,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p0\",\"reason\":\"completed\"}}\n",
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":1,\"promptId\":\"p1\",\"input\":[{{\"type\":\"text\",\"text\":{follow}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"KIMI-QUEUED-32299\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":1,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":1,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p1\",\"reason\":\"completed\"}}\n",
            ),
            original = serde_json::to_string(original).unwrap(),
            follow = serde_json::to_string(follow).unwrap(),
        );
        let capture =
            session_capture_lines(wire.lines().map(|line| Ok(line.to_owned())), 1, original)
                .unwrap()
                .expect("both turns completed");
        assert_eq!(
            capture.lead,
            Some(Outcome::Final("KIMI-BUSY-DONE-32299".into()))
        );
        assert_eq!(
            capture.steered,
            vec![(follow.to_owned(), "KIMI-QUEUED-32299".to_owned())]
        );
        assert_eq!(capture.continuation, None);
    }

    #[test]
    fn kimi_open_follow_up_turn_holds_the_lead() {
        let original = "[Bus message 5 from you]\nRun sleep 45 and reply KIMI-BUSY-DONE-32299.";
        let follow = "[Bus message 8 from you]\nReply with exactly KIMI-QUEUED-32299.";
        let wire = format!(
            concat!(
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":0,\"promptId\":\"p0\",\"input\":[{{\"type\":\"text\",\"text\":{original}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"KIMI-BUSY-DONE-32299\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":0,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":0,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p0\",\"reason\":\"completed\"}}\n",
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":1,\"promptId\":\"p1\",\"input\":[{{\"type\":\"text\",\"text\":{follow}}}]}}\n",
            ),
            original = serde_json::to_string(original).unwrap(),
            follow = serde_json::to_string(follow).unwrap(),
        );
        let capture =
            session_capture_lines(wire.lines().map(|line| Ok(line.to_owned())), 0, original)
                .unwrap()
                .expect("lead turn completed");
        assert_eq!(capture.lead, None);
        assert!(capture.steered.is_empty());
    }

    #[test]
    fn stale_open_bus_turn_does_not_hold_a_later_committed_reply() {
        let old = "[Bus message 5 from you]\nold request";
        let current = "[Bus message 8 from you]\ncurrent request";
        let wire = format!(
            concat!(
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":0,\"promptId\":\"p0\",\"input\":[{{\"type\":\"text\",\"text\":{old}}}]}}\n",
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":1,\"promptId\":\"p1\",\"input\":[{{\"type\":\"text\",\"text\":{current}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"CURRENT-REPLY\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":1,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":1,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p1\",\"reason\":\"completed\"}}\n",
            ),
            old = serde_json::to_string(old).unwrap(),
            current = serde_json::to_string(current).unwrap(),
        );
        let capture =
            session_capture_lines(wire.lines().map(|line| Ok(line.to_owned())), 1, current)
                .unwrap()
                .expect("current turn committed");
        assert_eq!(capture.lead, Some(Outcome::Final("CURRENT-REPLY".into())));
    }

    #[test]
    fn earlier_open_bus_turn_does_not_hide_a_later_completed_reply() {
        let earlier = "[Bus message 5 from you]\nFirst task.";
        let later = "[Bus message 8 from you]\nReply with exactly LATER-DONE.";
        let wire = format!(
            concat!(
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":0,\"promptId\":\"p0\",\"input\":[{{\"type\":\"text\",\"text\":{earlier}}}]}}\n",
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":1,\"promptId\":\"p1\",\"input\":[{{\"type\":\"text\",\"text\":{later}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"LATER-DONE\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":1,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":1,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p1\",\"reason\":\"completed\"}}\n",
            ),
            earlier = serde_json::to_string(earlier).unwrap(),
            later = serde_json::to_string(later).unwrap(),
        );
        let capture = session_capture_lines(wire.lines().map(|line| Ok(line.to_owned())), 1, later)
            .unwrap()
            .expect("later turn completed");
        assert_eq!(
            capture.lead,
            Some(Outcome::Final("LATER-DONE".into())),
            "an earlier Bus turn that never completed must not hide this turn's reply"
        );
    }

    #[test]
    fn answered_steer_does_not_hold_a_successful_silent_lead() {
        let original = "[Bus message 5 from you]\ncomplete silently";
        let steered = "[Bus message 8 from you]\nreply STEER-REPLY";
        let wire = format!(
            concat!(
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":0,\"promptId\":\"p0\",\"input\":[{{\"type\":\"text\",\"text\":{original}}}]}}\n",
                "{{\"type\":\"turn.steer\",\"agentId\":\"main\",\"turnId\":0,\"input\":[{{\"type\":\"text\",\"text\":{steered}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"STEER-REPLY\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":0,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":0,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p0\",\"reason\":\"completed\"}}\n",
            ),
            original = serde_json::to_string(original).unwrap(),
            steered = serde_json::to_string(steered).unwrap(),
        );
        let capture =
            session_capture_lines(wire.lines().map(|line| Ok(line.to_owned())), 0, original)
                .unwrap()
                .expect("lead turn committed");
        assert_eq!(capture.lead, Some(Outcome::NoReply));
        assert_eq!(
            capture.steered,
            vec![(steered.to_owned(), "STEER-REPLY".into())]
        );
    }

    #[test]
    fn nested_steer_in_a_follow_up_turn_gets_its_own_reply() {
        let original = "[Bus message 5 from you]\nlead";
        let first = "[Bus message 8 from you]\nfirst follow-up";
        let second = "[Bus message 9 from you]\nsecond follow-up";
        let wire = format!(
            concat!(
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":0,\"promptId\":\"p0\",\"input\":[{{\"type\":\"text\",\"text\":{original}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"LEAD-REPLY\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":0,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":0,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p0\",\"reason\":\"completed\"}}\n",
                "{{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":1,\"promptId\":\"p1\",\"input\":[{{\"type\":\"text\",\"text\":{first}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"FIRST-REPLY\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"turn.steer\",\"agentId\":\"main\",\"turnId\":1,\"input\":[{{\"type\":\"text\",\"text\":{second}}}]}}\n",
                "{{\"type\":\"agent.message.appended\",\"message\":{{\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"SECOND-REPLY\"}}],\"toolCalls\":[]}},\"meta\":{{\"source\":\"llm\"}}}}}}\n",
                "{{\"type\":\"agent.turn.ended\",\"turnId\":1,\"outcome\":\"done\"}}\n",
                "{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":1,\"reason\":\"completed\"}}\n",
                "{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p1\",\"reason\":\"completed\"}}\n",
            ),
            original = serde_json::to_string(original).unwrap(),
            first = serde_json::to_string(first).unwrap(),
            second = serde_json::to_string(second).unwrap(),
        );
        let capture =
            session_capture_lines(wire.lines().map(|line| Ok(line.to_owned())), 0, original)
                .unwrap()
                .expect("both turns committed");
        assert_eq!(capture.lead, Some(Outcome::Final("LEAD-REPLY".into())));
        assert_eq!(
            capture.steered,
            vec![
                (first.to_owned(), "FIRST-REPLY".into()),
                (second.to_owned(), "SECOND-REPLY".into()),
            ]
        );
    }

    #[test]
    fn malformed_wire_is_an_error_not_pending_evidence() {
        let lines = ["{not json".to_owned()];
        assert!(
            committed_outcome_lines(lines.map(Ok), 7, "Bus task").is_err(),
            "malformed evidence must not be mistaken for a running turn"
        );
    }
}
