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
}

fn committed_outcome_lines(
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
        if line.len() as u64 > MAX_WIRE_BYTES {
            return None;
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
                let input = value.get("input")?.as_array()?;
                let normalize = |value: &str| {
                    value
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .trim_end_matches(|character: char| character.is_ascii_whitespace())
                        .to_owned()
                };
                if input.len() != 1
                    || input[0].get("type").and_then(Value::as_str) != Some("text")
                    || input[0]
                        .get("text")
                        .and_then(Value::as_str)
                        .map(normalize)
                        .as_deref()
                        != Some(normalize(prompt).as_str())
                {
                    return None;
                }
                prompt_id = value.get("promptId")?.as_str()?.to_owned();
                if prompt_id.is_empty() {
                    return None;
                }
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
    Ok(committed_outcome_lines(lines, turn_id, prompt))
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
            "older history must not disable this committed turn's reply"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
