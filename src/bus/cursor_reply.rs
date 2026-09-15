//! Cursor's response hook may concatenate commentary and final-message text.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};

const MAX_TRANSCRIPT_TAIL: u64 = 2 * 1024 * 1024;
const PENDING: &str = "Awaiting Cursor's completed transcript to identify its final reply. The callback and request were kept; no prompt will be retried.";

pub(crate) fn final_text(value: &Value) -> Result<String, String> {
    let text = super::field(value, "text")?;
    let Some(path) = value.get("transcript_path").and_then(Value::as_str) else {
        // Older hook payloads expose only a response body.
        return Ok(text);
    };
    let mut file = std::fs::File::open(path).map_err(|_| PENDING.to_owned())?;
    let metadata = file.metadata().map_err(|_| PENDING.to_owned())?;
    if !metadata.is_file() {
        return Err(PENDING.into());
    }
    let offset = metadata.len().saturating_sub(MAX_TRANSCRIPT_TAIL);
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| PENDING.to_owned())?;
    let mut bytes = Vec::new();
    file.take(MAX_TRANSCRIPT_TAIL)
        .read_to_end(&mut bytes)
        .map_err(|_| PENDING.to_owned())?;
    let start = if offset == 0 {
        0
    } else {
        // A bounded tail can start inside a JSON record or UTF-8 character.
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .ok_or_else(|| PENDING.to_owned())?
    };
    let transcript = std::str::from_utf8(&bytes[start..]).map_err(|_| PENDING.to_owned())?;
    matching_completed_message(transcript, &text).ok_or_else(|| PENDING.to_owned())
}

fn matching_completed_message(transcript: &str, hook_text: &str) -> Option<String> {
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    let mut has_user = false;
    let mut direct_last = None;
    let mut matched = None;
    let mut ambiguous = false;
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line).ok()?;
        match value.get("role").and_then(Value::as_str) {
            Some("user") => {
                // Cursor can both inject user-shaped metadata within a generation
                // and replace an older generation's turn_ended marker when the
                // transcript advances. Preserve existing candidates while starting
                // another at every possible user boundary.
                observe_candidates(
                    &candidates,
                    direct_last.as_deref(),
                    hook_text,
                    &mut matched,
                    &mut ambiguous,
                );
                candidates.push((String::new(), None));
                has_user = true;
                direct_last = None;
            }
            Some("assistant") if has_user => {
                let content = value.get("message")?.get("content")?.as_array()?;
                let text = content
                    .iter()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<String>();
                // An assistant message containing a tool call is commentary.
                let final_text = (!text.trim().is_empty()
                    && !content
                        .iter()
                        .any(|part| part.get("type").and_then(Value::as_str) == Some("tool_use")))
                .then_some(text.clone());
                for (accumulated, last) in &mut candidates {
                    accumulated.push_str(&text);
                    *last = final_text.clone();
                }
                candidates.retain(|(accumulated, _)| hook_text.starts_with(accumulated.as_str()));
                direct_last = final_text;
            }
            _ => {}
        }
        if value.get("type").and_then(Value::as_str) == Some("turn_ended") {
            if value.get("status").and_then(Value::as_str) == Some("success") {
                observe_candidates(
                    &candidates,
                    direct_last.as_deref(),
                    hook_text,
                    &mut matched,
                    &mut ambiguous,
                );
            }
            candidates.clear();
            has_user = false;
            direct_last = None;
        }
    }
    (!ambiguous).then_some(matched).flatten()
}

fn observe_candidates(
    candidates: &[(String, Option<String>)],
    direct_last: Option<&str>,
    hook_text: &str,
    matched: &mut Option<String>,
    ambiguous: &mut bool,
) {
    for candidate in candidates
        .iter()
        .filter(|(accumulated, _)| accumulated == hook_text)
        .filter_map(|(_, last)| last.as_deref())
        .chain(direct_last.filter(|last| *last == hook_text))
    {
        // Callback identity already owns the request. Repeated matching turns
        // are safe if their extracted answers agree; conflicting
        // commentary/final splits remain ambiguous and fail closed.
        if matched
            .as_ref()
            .is_some_and(|existing| existing != candidate)
        {
            *ambiguous = true;
        } else if matched.is_none() {
            *matched = Some(candidate.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TURN: &str = concat!(
        "{\"role\":\"user\",\"message\":{\"content\":[]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Question first.\"},{\"type\":\"tool_use\",\"name\":\"AskQuestion\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"final\"},{\"type\":\"text\",\"text\":\" answer\"}]}}\n",
        "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
    );

    #[test]
    fn cursor_final_reply_excludes_question_commentary_and_keeps_message_blocks() {
        for hook in ["Question first.final answer", "final answer"] {
            assert_eq!(
                matching_completed_message(TURN, hook).as_deref(),
                Some("final answer")
            );
        }
    }

    #[test]
    fn cursor_final_reply_waits_for_completed_matching_turn() {
        assert!(matching_completed_message(TURN, "unrelated response").is_none());
        for incomplete in [
            TURN.replace("\"status\":\"success\"", "\"status\":\"error\""),
            TURN.lines().take(3).collect::<Vec<_>>().join("\n"),
            format!("{TURN}{{\"role\":\"assistant\""),
        ] {
            assert!(
                matching_completed_message(&incomplete, "Question first.final answer").is_none()
            );
        }
    }

    #[test]
    fn cursor_final_reply_matches_its_completed_turn_after_a_newer_turn_starts() {
        let newer = "{\"role\":\"user\",\"message\":{\"content\":[]}}\n";
        assert_eq!(
            matching_completed_message(&format!("{TURN}{newer}"), "Question first.final answer")
                .as_deref(),
            Some("final answer")
        );
        let different = TURN.replace("final", "newer");
        assert_eq!(
            matching_completed_message(
                &format!("{TURN}{different}"),
                "Question first.final answer"
            )
            .as_deref(),
            Some("final answer")
        );
    }

    #[test]
    fn cursor_final_reply_ignores_injected_user_records_within_completed_turn() {
        let transcript = concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"review\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Checking.\"},{\"type\":\"tool_use\",\"name\":\"Read\"}]}}\n",
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<available_subagent_types>...</available_subagent_types>\"}]}}\n",
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"review\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Ready\"}]}}\n",
            "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
        );

        assert_eq!(
            matching_completed_message(transcript, "Checking.Ready").as_deref(),
            Some("Ready")
        );
    }

    #[test]
    fn cursor_final_reply_recovers_prior_generation_after_transcript_advances() {
        let transcript = concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"older\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Older answer\"}]}}\n",
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"review\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Checking.\"},{\"type\":\"tool_use\",\"name\":\"Read\"}]}}\n",
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<available_subagent_types>...</available_subagent_types>\"}]}}\n",
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"review\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Ready\"}]}}\n",
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"newer\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Newer answer\"}]}}\n",
            "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
        );

        assert_eq!(
            matching_completed_message(transcript, "Checking.Ready").as_deref(),
            Some("Ready")
        );
    }

    #[test]
    fn cursor_final_reply_allows_repeated_identical_answers_but_not_conflicting_splits() {
        let ok = TURN
            .lines()
            .filter(|line| !line.contains("Question first."))
            .collect::<Vec<_>>()
            .join("\n")
            .replace("\"final\"", "\"O\"")
            .replace("\" answer\"", "\"K\"");
        assert_eq!(
            matching_completed_message(&format!("{ok}\n{ok}\n"), "OK").as_deref(),
            Some("OK")
        );
        assert_eq!(
            matching_completed_message(&format!("{TURN}{TURN}"), "Question first.final answer")
                .as_deref(),
            Some("final answer")
        );
        // Same aggregate text, but different commentary/final boundaries.
        let conflicting = TURN
            .replace("Question first.", "Question ")
            .replace("\"final\"", "\"first.final\"");
        assert!(matching_completed_message(
            &format!("{TURN}{conflicting}"),
            "Question first.final answer"
        )
        .is_none());
    }
}
