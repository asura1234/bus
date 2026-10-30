//! Kimi 2.4.0's private wire is the completion witness. Stop is only a wake.
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_WIRE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn committed_final(wire: &str, turn_id: u64, prompt: &str) -> Option<String> {
    let mut owned = false;
    let mut prompt_id = String::new();
    let mut answer = None;
    let mut agent_done = false;
    let mut turn_done = false;
    for line in wire.lines() {
        let value: Value = serde_json::from_str(line).ok()?;
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
                if input.len() != 1
                    || input[0].get("type").and_then(Value::as_str) != Some("text")
                    || input[0].get("text").and_then(Value::as_str) != Some(prompt)
                {
                    return None;
                }
                prompt_id = value.get("promptId")?.as_str()?.to_owned();
                if prompt_id.is_empty() {
                    return None;
                }
                owned = true;
            }
            "agent.message.appended" if owned && !agent_done => {
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
                    if value.get("outcome").and_then(Value::as_str) != Some("done") {
                        return None;
                    }
                    agent_done = true;
                }
            }
            "turn.ended" if owned => {
                if value.get("agentId").and_then(Value::as_str) == Some("main")
                    && value.get("turnId").and_then(Value::as_u64) == Some(turn_id)
                {
                    if !agent_done
                        || value.get("reason").and_then(Value::as_str) != Some("completed")
                    {
                        return None;
                    }
                    turn_done = true;
                }
            }
            "prompt.completed" if owned => {
                if value.get("agentId").and_then(Value::as_str) == Some("main")
                    && value.get("promptId").and_then(Value::as_str) == Some(&prompt_id)
                {
                    if !turn_done
                        || value.get("reason").and_then(Value::as_str) != Some("completed")
                    {
                        return None;
                    }
                    return answer;
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn resolve(
    home: &Path,
    cwd: &Path,
    session: &str,
    turn: &str,
    prompt: &str,
) -> Result<Option<String>, String> {
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
    let bytes = match std::fs::metadata(&wire) {
        Ok(meta) if meta.len() <= MAX_WIRE_BYTES => {
            std::fs::read(&wire).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err("Kimi wire is oversized".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let wire = String::from_utf8(bytes).map_err(|_| "Kimi wire is not UTF-8")?;
    Ok(committed_final(&wire, turn_id, prompt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_requires_its_private_session_cwd_and_committed_turn() {
        let root = crate::utils::test_temp::unique_temp_path("kimi-wire-resolve");
        let cwd = root.join("project");
        let session = "session_test";
        let directory = root.join("sessions/wd_project/session_test");
        crate::messaging::storage::io::private_dir(&cwd).unwrap();
        crate::messaging::storage::io::private_dir(&directory.join("agents/main")).unwrap();
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
}
