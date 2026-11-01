//! Kimi 2.4.0's private wire is the completion witness. Stop is only a wake.
use serde_json::Value;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_WIRE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    Final(String),
    NoReply,
    Failed(String),
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

fn owned_prompt_id(value: &Value, prompt: &str) -> Option<String> {
    let input = value.get("input")?.as_array()?;
    if input.len() != 1
        || input[0].get("type").and_then(Value::as_str) != Some("text")
        || input[0]
            .get("text")
            .and_then(Value::as_str)
            .map(normalized_prompt)
            .as_deref()
            != Some(normalized_prompt(prompt).as_str())
    {
        return None;
    }
    let id = value.get("promptId")?.as_str()?;
    (!id.is_empty()).then(|| id.to_owned())
}

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
    let outcome = scan_committed_outcome(checked, turn_id, prompt);
    if let Some(error) = invalid {
        return Err(error);
    }
    Ok(outcome)
}

fn scan_committed_outcome(
    lines: impl IntoIterator<Item = Result<String, String>>,
    turn_id: u64,
    prompt: &str,
) -> Option<Outcome> {
    let mut owned = false;
    let mut prompt_id = String::new();
    let mut answer = None;
    let mut agent_outcome = None;
    let mut agent_error = None;
    let mut turn_reason = None;
    for line in lines {
        let line = line.ok()?;
        // One oversized historical line must not hide a later committed turn.
        if line.len() as u64 > MAX_WIRE_BYTES {
            continue;
        }
        let value: Value = serde_json::from_str(&line).ok()?;
        match value.get("type")?.as_str()? {
            "turn.prompt" => {
                if owned {
                    return None;
                }
                if value.get("agentId").and_then(Value::as_str) != Some("main")
                    || value.get("turnId").and_then(Value::as_u64) != Some(turn_id)
                {
                    continue;
                }
                prompt_id = owned_prompt_id(&value, prompt)?;
                owned = true;
            }
            "agent.message.appended" if owned && agent_outcome.is_none() => {
                let message = value.get("message")?.get("message")?;
                if message.get("role").and_then(Value::as_str) != Some("assistant")
                    || value
                        .pointer("/message/meta/source")
                        .and_then(Value::as_str)
                        != Some("llm")
                {
                    continue;
                }
                if message
                    .get("toolCalls")
                    .and_then(Value::as_array)
                    .is_some_and(|calls| !calls.is_empty())
                {
                    answer = None;
                    continue;
                }
                let text = message
                    .get("content")?
                    .as_array()?
                    .iter()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                    .map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Option<Vec<_>>>()?
                    .concat();
                if !text.is_empty() {
                    answer = Some(text);
                }
            }
            "agent.turn.ended" if owned => {
                if value.get("turnId").and_then(Value::as_u64) == Some(turn_id) {
                    agent_outcome = Some(value.get("outcome")?.as_str()?.to_owned());
                    agent_error = value
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            "turn.ended" if owned => {
                if value.get("agentId").and_then(Value::as_str) == Some("main")
                    && value.get("turnId").and_then(Value::as_u64) == Some(turn_id)
                {
                    agent_outcome.as_ref()?;
                    turn_reason = Some(value.get("reason")?.as_str()?.to_owned());
                }
            }
            "prompt.completed"
                if owned
                    && value.get("agentId").and_then(Value::as_str) == Some("main")
                    && value.get("promptId").and_then(Value::as_str) == Some(&prompt_id) =>
            {
                return terminal_outcome(
                    agent_outcome.as_deref(),
                    turn_reason.as_deref(),
                    value.get("reason")?.as_str()?,
                    answer,
                    agent_error,
                );
            }
            _ => {}
        }
    }
    None
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

pub(crate) fn resolve_outcome(
    home: &Path,
    cwd: &Path,
    session: &str,
    turn: &str,
    prompt: &str,
) -> Result<Option<Outcome>, String> {
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
    committed_outcome_lines(lines, turn_id, prompt)
}

/// Latest main-agent turn in this Kimi home, independent of reply text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MainTurn {
    /// `turn.prompt` has no completed terminal chain yet.
    Open,
    /// `agent.turn.ended`, `turn.ended`, and `prompt.completed` agree.
    Settled,
}

#[derive(Clone, Debug)]
struct WireStamp {
    path: PathBuf,
    present: bool,
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MainTurnCache {
    home: Option<PathBuf>,
    valid: bool,
    index_exists: bool,
    index_len: u64,
    index_modified: Option<SystemTime>,
    wires: Vec<WireStamp>,
    turn: Option<MainTurn>,
}

struct IndexedWire {
    path: PathBuf,
    stamp: Option<(u64, Option<SystemTime>)>,
}

/// Read the newest main wire when the index or a wire stamp changed.
pub(crate) fn refresh_main_turn(
    home: &Path,
    cache: &mut MainTurnCache,
) -> Result<Option<MainTurn>, String> {
    let home = home.canonicalize().map_err(|error| error.to_string())?;
    let index = home.join("session_index.jsonl");
    let index_stamp = regular_stamp(&index, Some(MAX_INDEX_BYTES))?;
    let Some((index_len, index_modified)) = index_stamp else {
        *cache = MainTurnCache {
            home: Some(home),
            valid: true,
            ..MainTurnCache::default()
        };
        return Ok(None);
    };
    if cache.valid
        && cache.home.as_deref() == Some(home.as_path())
        && cache.index_exists
        && cache.index_len == index_len
        && cache.index_modified == index_modified
        && cache.wires.iter().all(wire_stamp_unchanged)
    {
        return Ok(cache.turn);
    }
    let indexed = indexed_wires(&home, &index)?;
    let mut newest: Option<(SystemTime, u64, usize)> = None;
    for (index_of_wire, wire) in indexed.iter().enumerate() {
        let Some((len, modified)) = wire.stamp else {
            continue;
        };
        let modified_key = modified.unwrap_or(SystemTime::UNIX_EPOCH);
        let replace = match &newest {
            None => true,
            Some((previous, previous_len, _)) => {
                modified_key > *previous || (modified_key == *previous && len >= *previous_len)
            }
        };
        if replace {
            newest = Some((modified_key, len, index_of_wire));
        }
    }
    let turn = match newest {
        Some((_, _, index_of_wire)) => main_turn_in_file(&indexed[index_of_wire].path)?,
        None => None,
    };
    *cache = MainTurnCache {
        home: Some(home),
        valid: true,
        index_exists: true,
        index_len,
        index_modified,
        wires: indexed
            .into_iter()
            .map(|wire| WireStamp {
                path: wire.path,
                present: wire.stamp.is_some(),
                len: wire.stamp.map(|(len, _)| len).unwrap_or(0),
                modified: wire.stamp.and_then(|(_, modified)| modified),
            })
            .collect(),
        turn,
    };
    Ok(turn)
}

fn wire_stamp_unchanged(stamp: &WireStamp) -> bool {
    match std::fs::symlink_metadata(&stamp.path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => !stamp.present,
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
            stamp.present && meta.len() == stamp.len && meta.modified().ok() == stamp.modified
        }
        _ => false,
    }
}

fn regular_stamp(
    path: &Path,
    max_len: Option<u64>,
) -> Result<Option<(u64, Option<SystemTime>)>, String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if meta.file_type().is_symlink() {
        return Err("Kimi session file is a symlink".into());
    }
    if !meta.is_file() {
        return Err("Kimi session file is not a regular file".into());
    }
    if max_len.is_some_and(|max| meta.len() > max) {
        return Err("Kimi session index is oversized".into());
    }
    Ok(Some((meta.len(), meta.modified().ok())))
}

fn indexed_wires(home: &Path, index: &Path) -> Result<Vec<IndexedWire>, String> {
    let bytes = std::fs::read(index).map_err(|error| error.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "Kimi session index is not UTF-8")?;
    let mut wires = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let record: Value =
            serde_json::from_str(line).map_err(|_| "Kimi session index is incomplete")?;
        let Some(session) = record.get("sessionId").and_then(Value::as_str) else {
            continue;
        };
        let Some(directory) = record.get("sessionDir").and_then(Value::as_str) else {
            continue;
        };
        let directory = PathBuf::from(directory);
        if !directory.is_absolute() {
            continue;
        }
        let Ok(directory) = directory.canonicalize() else {
            continue;
        };
        if !directory.starts_with(home)
            || directory.file_name().and_then(|name| name.to_str()) != Some(session)
        {
            return Err("Kimi session directory escaped its private home".into());
        }
        let wire = directory.join("agents/main/wire.jsonl");
        if std::fs::symlink_metadata(&wire).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err("Kimi wire is a symlink".into());
        }
        wires.push(IndexedWire {
            stamp: regular_stamp(&wire, None)?,
            path: wire,
        });
    }
    Ok(wires)
}

fn main_turn_in_file(path: &Path) -> Result<Option<MainTurn>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let lines = std::io::BufReader::new(file)
        .lines()
        .map(|line| line.map_err(|error| error.to_string()));
    main_turn_lines(lines)
}

fn main_turn_lines(
    lines: impl IntoIterator<Item = Result<String, String>>,
) -> Result<Option<MainTurn>, String> {
    let mut invalid = None;
    let mut current: Option<OpenTurn> = None;
    for line in lines {
        let line = line?;
        if line.len() as u64 > MAX_WIRE_BYTES || line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                invalid = Some("Kimi wire contains malformed JSON".to_string());
                break;
            }
        };
        observe_main_turn(&value, &mut current);
    }
    if let Some(error) = invalid {
        return Err(error);
    }
    Ok(current.map(|turn| {
        if terminal_outcome(
            turn.agent.as_deref(),
            turn.turn_reason.as_deref(),
            turn.prompt_reason.as_deref().unwrap_or(""),
            None,
            None,
        )
        .is_some()
        {
            MainTurn::Settled
        } else {
            MainTurn::Open
        }
    }))
}

struct OpenTurn {
    turn_id: u64,
    prompt_id: String,
    agent: Option<String>,
    turn_reason: Option<String>,
    prompt_reason: Option<String>,
}

fn observe_main_turn(value: &Value, current: &mut Option<OpenTurn>) {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "turn.prompt" => {
            if value.get("agentId").and_then(Value::as_str) != Some("main") {
                return;
            }
            let Some(turn_id) = value.get("turnId").and_then(Value::as_u64) else {
                return;
            };
            let Some(prompt_id) = value.get("promptId").and_then(Value::as_str) else {
                return;
            };
            if prompt_id.is_empty() {
                return;
            }
            *current = Some(OpenTurn {
                turn_id,
                prompt_id: prompt_id.to_owned(),
                agent: None,
                turn_reason: None,
                prompt_reason: None,
            });
        }
        "agent.turn.ended" => {
            let Some(turn) = current.as_mut() else {
                return;
            };
            if value.get("turnId").and_then(Value::as_u64) != Some(turn.turn_id) {
                return;
            }
            if let Some(outcome) = value.get("outcome").and_then(Value::as_str) {
                turn.agent = Some(outcome.to_owned());
            }
        }
        "turn.ended" => {
            let Some(turn) = current.as_mut() else {
                return;
            };
            if turn.agent.is_none()
                || value.get("agentId").and_then(Value::as_str) != Some("main")
                || value.get("turnId").and_then(Value::as_u64) != Some(turn.turn_id)
            {
                return;
            }
            if let Some(reason) = value.get("reason").and_then(Value::as_str) {
                turn.turn_reason = Some(reason.to_owned());
            }
        }
        "prompt.completed" => {
            let Some(turn) = current.as_mut() else {
                return;
            };
            if value.get("agentId").and_then(Value::as_str) != Some("main")
                || value.get("promptId").and_then(Value::as_str) != Some(turn.prompt_id.as_str())
            {
                return;
            }
            if let Some(reason) = value.get("reason").and_then(Value::as_str) {
                turn.prompt_reason = Some(reason.to_owned());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
fn main_turn(wire: &str) -> Result<Option<MainTurn>, String> {
    main_turn_lines(wire.lines().map(|line| Ok(line.to_owned())))
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
    fn malformed_wire_is_an_error_not_pending_evidence() {
        let lines = ["{not json".to_owned()];
        assert!(
            committed_outcome_lines(lines.map(Ok), 7, "Bus task").is_err(),
            "malformed evidence must not be mistaken for a running turn"
        );
    }

    fn turn_chain(done: bool) -> String {
        let mut wire = concat!(
            "{\"type\":\"agent.turn.started\",\"turnId\":3}\n",
            "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":3,\"promptId\":\"msg_01M6AC910P8RJ3RQA02GHYV19P\",\"input\":[{\"type\":\"text\",\"text\":\"Reply with exactly this text and nothing else: Running a command\"}]}\n",
            "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Running a command\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        )
        .to_owned();
        if done {
            wire.push_str(concat!(
                "{\"type\":\"agent.turn.ended\",\"turnId\":3,\"outcome\":\"done\"}\n",
                "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":3,\"reason\":\"completed\"}\n",
                "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"msg_01M6AC910P8RJ3RQA02GHYV19P\",\"reason\":\"completed\"}\n",
            ));
        }
        wire
    }

    #[test]
    fn kimi_latest_main_turn_settles_only_after_the_terminal_chain() {
        assert_eq!(main_turn(&turn_chain(false)).unwrap(), Some(MainTurn::Open));
        assert_eq!(
            main_turn(&turn_chain(true)).unwrap(),
            Some(MainTurn::Settled),
            "a finished reply can still say Running a command; the turn chain is the signal"
        );
        let mut reopened = turn_chain(true);
        reopened.push_str(
            "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":4,\"promptId\":\"next\",\"input\":[{\"type\":\"text\",\"text\":\"again\"}]}\n",
        );
        assert_eq!(main_turn(&reopened).unwrap(), Some(MainTurn::Open));
    }

    #[test]
    fn kimi_refresh_does_not_reuse_another_homes_turn() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-home-switch");
        let homes = [root.join("open"), root.join("done")];
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        for (home, settled) in homes.iter().zip([false, true]) {
            let directory = home.join("sessions/wd/session_1");
            crate::platform::fs::private_dir(&directory.join("agents/main")).unwrap();
            let index = serde_json::json!({"sessionId":"session_1","sessionDir":directory});
            let index_path = home.join("session_index.jsonl");
            std::fs::write(&index_path, format!("{index}\n")).unwrap();
            std::fs::File::options()
                .write(true)
                .open(index_path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
            std::fs::write(
                directory.join("agents/main/wire.jsonl"),
                turn_chain(settled),
            )
            .unwrap();
        }
        let mut cache = MainTurnCache::default();
        assert_eq!(
            refresh_main_turn(&homes[0], &mut cache).unwrap(),
            Some(MainTurn::Open)
        );
        let actual = refresh_main_turn(&homes[1], &mut cache).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(
            actual,
            Some(MainTurn::Settled),
            "a native relaunch must read its own home even with identical index stamps"
        );
    }

    #[test]
    fn kimi_refresh_reads_the_newest_main_wire_under_the_private_home() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-main-turn");
        let session = "session_519102dd-97b9-49ef-8c36-3d84b5a89afd";
        let directory = root.join("sessions/wd").join(session);
        crate::platform::fs::private_dir(&directory.join("agents/main")).unwrap();
        let index = serde_json::json!({
            "sessionId": session,
            "sessionDir": directory,
        });
        std::fs::write(root.join("session_index.jsonl"), format!("{index}\n")).unwrap();
        let wire_path = directory.join("agents/main/wire.jsonl");
        std::fs::write(&wire_path, turn_chain(false)).unwrap();
        let mut cache = MainTurnCache::default();
        assert_eq!(
            refresh_main_turn(&root, &mut cache).unwrap(),
            Some(MainTurn::Open)
        );
        assert_eq!(
            refresh_main_turn(&root, &mut cache).unwrap(),
            Some(MainTurn::Open),
            "an unchanged wire is not reread into a different turn"
        );
        std::fs::write(&wire_path, turn_chain(true)).unwrap();
        assert_eq!(
            refresh_main_turn(&root, &mut cache).unwrap(),
            Some(MainTurn::Settled)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
