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
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            "Hook missing text; update the CLI and verify Bus hooks. No prompt will be retried."
                .to_owned()
        })?;
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
/// The prompt is the short task notice itself, not a developer request that
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
    // Only Cursor's leading wrapper; a developer message may quote one mid-text.
    if let Some((_, rest)) = text
        .strip_prefix("<timestamp>")
        .and_then(|rest| rest.split_once("</timestamp>"))
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
#[path = "tests/final_reply_test.rs"]
mod tests;
