//! A dev-tools-only adapter for the synthetic transcript contract. Never a
//! claim about native Grok serialization. The coordinator supplies an owned
//! scratch root; native JSON cannot opt itself into this adapter.
use super::completion::CompletionAttestation;
use std::path::Path;

pub(crate) fn attest(
    scratch: Option<&Path>,
    path: &Path,
    session: &str,
    turn: &str,
) -> Result<Option<CompletionAttestation>, String> {
    use serde_json::Value;
    use std::io::Read;
    let Some(scratch) = scratch else {
        return Ok(None);
    };
    let workspace = scratch
        .join("work")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let canonical = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !canonical.starts_with(workspace)
        || canonical.file_name().is_none_or(|n| n != "updates.jsonl")
        || canonical
            .parent()
            .and_then(Path::file_name)
            .is_none_or(|n| n != session)
        || canonical
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_none_or(|n| n != "bus-fake")
        || std::fs::symlink_metadata(path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err("Synthetic Grok transcript is outside its owned scratch session".into());
    }
    let file = std::fs::File::open(canonical).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 2 * 1024 * 1024 || bytes.last() != Some(&b'\n') {
        return Ok(None);
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };
    let (mut began, mut completed, mut reply, mut latest) =
        (false, false, String::new(), String::new());
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            return Ok(None);
        };
        if row["method"] != "_x.ai/session/update"
            || row["params"]["sessionId"] != session
            || row["params"]["_meta"]["synthetic"] != true
        {
            return Ok(None);
        }
        let update = &row["params"]["update"];
        let Some(id) = update["promptId"].as_str() else {
            return Ok(None);
        };
        if row["params"]["_meta"]["promptId"] != id {
            return Ok(None);
        }
        if update["sessionUpdate"] == "turn_start" {
            latest = id.to_owned();
        }
        if id != turn {
            continue;
        }
        match update["sessionUpdate"].as_str() {
            Some("turn_start") if !began => began = true,
            Some("agent_message_chunk") if began && !completed => {
                let Some(chunk) = update["content"]["text"].as_str() else {
                    return Ok(None);
                };
                reply.push_str(chunk);
            }
            Some("turn_complete") if began && !completed && update["stopReason"] == "end_turn" => {
                completed = true
            }
            Some("turn_complete" | "turn_start") => return Ok(None),
            _ => {}
        }
    }
    Ok(
        (began && completed && latest == turn).then(|| CompletionAttestation {
            session: session.into(),
            turn: turn.into(),
            text: reply,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grok_synthetic_attestation_needs_owned_root_and_committed_terminal_record() {
        let root = crate::utils::test_temp::unique_temp_path("grok-synthetic-attestation");
        let path = root.join("work/.grok/bus-fake/session/updates.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let row = |update| {
            json!({"method":"_x.ai/session/update", "params":{"sessionId":"session", "update":update, "_meta":{"synthetic":true,"promptId":"turn"}}}).to_string()+"\n"
        };
        let start = row(json!({"sessionUpdate":"turn_start","promptId":"turn"}));
        let message = row(
            json!({"sessionUpdate":"agent_message_chunk","promptId":"turn","content":{"type":"text","text":"full 界\nreply"}}),
        );
        let end =
            row(json!({"sessionUpdate":"turn_complete","promptId":"turn","stopReason":"end_turn"}));
        std::fs::write(&path, format!("{start}{message}{end}")).unwrap();
        assert!(
            attest(None, &path, "session", "turn").unwrap().is_none(),
            "JSON cannot authorize synthetic evidence"
        );
        let final_ = attest(Some(&root), &path, "session", "turn")
            .unwrap()
            .expect("trusted synthetic final");
        assert_eq!(final_.text, "full 界\nreply");
        assert!(attest(Some(&root), &path, "session", "other")
            .unwrap()
            .is_none());
        std::fs::write(&path, format!("{start}{message}{{")).unwrap();
        assert!(attest(Some(&root), &path, "session", "turn")
            .unwrap()
            .is_none());
        std::fs::write(
            &path,
            format!("{start}{message}{}", end.replace("end_turn", "cancelled")),
        )
        .unwrap();
        assert!(attest(Some(&root), &path, "session", "turn")
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
