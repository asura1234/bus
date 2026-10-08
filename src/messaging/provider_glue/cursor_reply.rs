//! Cursor's response hook may concatenate commentary and final-message text.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};

const MAX_TRANSCRIPT_TAIL: u64 = 2 * 1024 * 1024;
const PENDING: &str = "Awaiting Cursor's completed transcript to identify its final reply. The callback and request were kept; no prompt will be retried.";

fn read_transcript(path: &str) -> Result<String, String> {
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
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .ok_or_else(|| PENDING.to_owned())?
    };
    std::str::from_utf8(&bytes[start..])
        .map(|text| text.to_owned())
        .map_err(|_| PENDING.to_owned())
}

/// Reply for a Cursor stop that already arrived as completed.
/// The transcript match wins. When it fails and the agent is idle, the hook's
/// own text is the reply: the turn is over, and waiting for a perfect
/// transcript match leaves the request delivered forever.
pub(crate) fn settle_text(value: &Value, idle: bool) -> Result<String, String> {
    let text = super::field(value, "text")?;
    let Some(path) = value.get("transcript_path").and_then(Value::as_str) else {
        return Ok(text);
    };
    let transcript = read_transcript(path)?;
    match classify(&transcript, &text) {
        ReplyMatch::Found(reply) => Ok(reply),
        // 转录还是钩子的前缀，说明写盘还没赶上，先留着再读。
        ReplyMatch::Waiting => Err(PENDING.into()),
        // 已经对不上，且代理空闲：用钩子正文结算，避免请求一直停在 delivered。
        ReplyMatch::Diverged if idle && !text.trim().is_empty() => Ok(text),
        ReplyMatch::Diverged => Err(PENDING.into()),
    }
}

enum ReplyMatch {
    Found(String),
    Waiting,
    Diverged,
}

/// Cursor's own wake after a background shell, not a Bus message.
/// The prompt is the short task notice itself, not a human request that
/// happens to quote one.
pub(crate) fn is_background_task_notice(prompt: &str) -> bool {
    let text = notice_body(prompt);
    if text.len() > 500 {
        return false;
    }
    text.starts_with("Briefly inform the user about the task result")
        || (text.starts_with("Finished ") && !text.contains('\n'))
}

fn notice_body(prompt: &str) -> &str {
    let mut text = prompt.trim();
    if let Some(rest) = text
        .find("</timestamp>")
        .map(|index| &text[index + "</timestamp>".len()..])
    {
        text = rest.trim();
    }
    if let Some(inner) = text
        .strip_prefix("<user_query>")
        .and_then(|rest| rest.strip_suffix("</user_query>"))
    {
        text = inner.trim();
    }
    text
}

#[cfg(test)]
fn matching_completed_message(transcript: &str, hook_text: &str) -> Option<String> {
    match classify(transcript, hook_text) {
        ReplyMatch::Found(text) => Some(text),
        ReplyMatch::Waiting | ReplyMatch::Diverged => None,
    }
}

fn classify(transcript: &str, hook_text: &str) -> ReplyMatch {
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    let mut has_user = false;
    let mut direct_last = None;
    let mut matched = None;
    let mut ambiguous = false;
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return ReplyMatch::Waiting;
        };
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
                let Some(content) = value
                    .get("message")
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_array)
                else {
                    return ReplyMatch::Waiting;
                };
                let text = content
                    .iter()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<String>();
                // 带工具调用的是说明，不是终稿。说明末尾可能多出钩子删掉的内容。
                let commentary = content
                    .iter()
                    .any(|part| part.get("type").and_then(Value::as_str) == Some("tool_use"));
                let final_text = (!text.trim().is_empty() && !commentary).then_some(text.clone());
                let mut next = Vec::new();
                for (accumulated, _) in &candidates {
                    let piece = if commentary {
                        commentary_kept(accumulated, &text, hook_text)
                    } else {
                        text.as_str()
                    };
                    let accumulated = format!("{accumulated}{piece}");
                    if hook_text.starts_with(&accumulated) {
                        next.push((accumulated, final_text.clone()));
                    }
                }
                candidates = next;
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
    // stop 已报完成才会来这里。这份 jsonl 经常不写 turn_ended，回复停在
    // 最后一条 assistant 上；不在结尾结算的话请求会一直停在 delivered。
    let waiting = candidates
        .iter()
        .any(|(accumulated, _)| hook_text.starts_with(accumulated) && accumulated != hook_text);
    observe_candidates(
        &candidates,
        direct_last.as_deref(),
        hook_text,
        &mut matched,
        &mut ambiguous,
    );
    if ambiguous {
        return ReplyMatch::Waiting;
    }
    if let Some(text) = matched {
        return ReplyMatch::Found(text);
    }
    if waiting {
        ReplyMatch::Waiting
    } else {
        ReplyMatch::Diverged
    }
}

/// 说明文字里钩子没带上的后缀（例如转录多写的标记）不参与对齐。
/// 终稿不走这里，避免把答案截短后误配上。
fn commentary_kept<'a>(accumulated: &str, text: &'a str, hook_text: &str) -> &'a str {
    if !hook_text.starts_with(accumulated) {
        return text;
    }
    let rest = &hook_text[accumulated.len()..];
    if rest.starts_with(text) {
        return text;
    }
    let bytes = text
        .char_indices()
        .zip(rest.chars())
        .take_while(|((_, transcript), hook)| transcript == hook)
        .last()
        .map(|((index, ch), _)| index + ch.len_utf8())
        .unwrap_or(0);
    &text[..bytes]
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

    /// 消息 1422：后续问题的说明块比钩子多一段后缀，且 jsonl 没有 turn_ended。
    #[test]
    fn cursor_final_reply_ignores_commentary_suffix_the_hook_omits() {
        for omitted in ["\\n\\n[REDACTED]", "\\n\\n<dropped>"] {
            let transcript = format!(
                "{{\"role\":\"user\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"research\"}}]}}}}\n\
                 {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"I'll check.\"}},{{\"type\":\"tool_use\",\"name\":\"Shell\"}}]}}}}\n\
                 {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"First answer.\"}}]}}}}\n\
                 {{\"role\":\"user\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"also search the web\"}}]}}}}\n\
                 {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"I'll search the docs.{omitted}\"}},{{\"type\":\"tool_use\",\"name\":\"CallDynamicTool\"}}]}}}}\n\
                 {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"name\":\"Grep\"}}]}}}}\n\
                 {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"Second answer.\"}}]}}}}\n"
            );
            assert_eq!(
                matching_completed_message(&transcript, "I'll search the docs.Second answer.")
                    .as_deref(),
                Some("Second answer."),
                "{omitted}"
            );
            assert_eq!(
                matching_completed_message(&transcript, "I'll check.First answer.").as_deref(),
                Some("First answer."),
                "{omitted}"
            );
        }
        // 终稿里的同样字样属于答案，钩子带上了就必须原样匹配。
        let kept = concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"see [REDACTED]\"}]}}\n",
        );
        assert_eq!(
            matching_completed_message(kept, "see [REDACTED]").as_deref(),
            Some("see [REDACTED]")
        );
    }

    #[test]
    fn cursor_final_reply_waits_for_completed_matching_turn() {
        assert!(matching_completed_message(TURN, "unrelated response").is_none());
        for incomplete in [
            TURN.replace("\"status\":\"success\"", "\"status\":\"error\""),
            TURN.lines().take(2).collect::<Vec<_>>().join("\n"),
            format!("{TURN}{{\"role\":\"assistant\""),
        ] {
            assert!(
                matching_completed_message(&incomplete, "Question first.final answer").is_none()
            );
        }
        // 终稿已经在文件里、只是没有 turn_ended 时也要结算。
        assert_eq!(
            matching_completed_message(
                &TURN.lines().take(3).collect::<Vec<_>>().join("\n"),
                "Question first.final answer"
            )
            .as_deref(),
            Some("final answer")
        );
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

    fn hook_value(text: &str, transcript: &str) -> Value {
        let path = std::env::temp_dir().join(format!(
            "bus-cursor-settle-{}-{}.jsonl",
            std::process::id(),
            text.len() + transcript.len()
        ));
        std::fs::write(&path, transcript).unwrap();
        serde_json::json!({"text": text, "transcript_path": path})
    }

    #[test]
    fn idle_completed_stop_uses_the_hook_text_when_the_transcript_does_not_match() {
        // 消息 1422 的钩子形状：说明和终稿粘在一起，转录对不上。
        let hook = "I'll search.The published docs still have no launch-time system prompt.";
        let value = hook_value(
            hook,
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"addendum\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"unrelated\"}]}}\n",
            ),
        );
        assert!(settle_text(&value, false).is_err());
        assert_eq!(settle_text(&value, true).as_deref(), Ok(hook));
    }

    #[test]
    fn a_matching_transcript_still_beats_the_raw_hook_text() {
        let value = hook_value(
            "I'll search.The published docs still have no launch-time system prompt.",
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"addendum\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I'll search.\\n\\n[REDACTED]\"},{\"type\":\"tool_use\",\"name\":\"WebSearch\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"The published docs still have no launch-time system prompt.\"}]}}\n",
            ),
        );
        assert_eq!(
            settle_text(&value, true).as_deref(),
            Ok("The published docs still have no launch-time system prompt.")
        );
    }

    #[test]
    fn background_task_notices_are_not_room_prompts() {
        let wake = "<timestamp>Wednesday, Oct 7, 2026, 8:30 PM (UTC+8)</timestamp>  <user_query>Briefly inform the user about the task result and perform any follow-up actions (if needed). If there's no follow-ups needed, don't explicitly say that.</user_query>";
        assert!(is_background_task_notice(wake));
        assert!(is_background_task_notice(
            "Finished Resume session after changing the ancestor rule"
        ));
        assert!(!is_background_task_notice(
            "Bug fix task. The notice was: Briefly inform the user about the task result. Then fix the matcher."
        ));
        assert!(!is_background_task_notice(
            "Finished the design.\n\nHere is the rest of the work."
        ));
    }
}
