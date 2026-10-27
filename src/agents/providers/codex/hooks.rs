//! Codex hook decoding; only persisted interactive sessions emit observations.
use super::super::spool::{field, optional_text, Parsed};
use serde_json::Value;

/// Codex attaches an image only when its quoted path is pasted on its own.
/// Keep every other line verbatim for a separate text paste and hook matching.
pub(crate) fn image_paste_parts(text: &str) -> (Vec<&str>, String) {
    let mut images = Vec::new();
    let remaining = text
        .split('\n')
        .filter(|line| {
            let path = line.strip_suffix('\r').unwrap_or(line);
            if super::super::claude_code::hooks::claude_lifted_images(path).0 == 1 {
                images.push(path);
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (images, remaining)
}

/// Codex separates image chips, and the text after them, with one space in its
/// submit hook (0.164.0). Remove only those separators, preserving body whitespace.
pub(crate) fn image_prompt_parts(mut prompt: &str) -> (usize, &str) {
    let mut count = 0;
    loop {
        let (length, images) = super::super::claude_code::hooks::claude_image_placeholders(prompt);
        if images == 0 {
            return (count, prompt);
        }
        count += images;
        prompt = &prompt[length..];
        match prompt.strip_prefix(' ') {
            Some(next) => prompt = next,
            None => return (count, prompt),
        }
    }
}

pub(crate) fn parse(value: &Value) -> Result<Parsed, String> {
    let event = field(value, "hook_event_name")?;
    // Title/memory sessions inherit hooks but have no interactive transcript.
    // Keep this before decoding their otherwise irrelevant session/turn fields.
    if value
        .get("transcript_path")
        .and_then(Value::as_str)
        .is_none_or(|path| path.is_empty())
    {
        return Ok(Parsed::Ignore);
    }
    if value.get("agent_id").is_some_and(|v| !v.is_null()) || event.starts_with("Subagent") {
        return Ok(Parsed::Ignore);
    }
    let session = field(value, "session_id")?;
    if event == "SessionStart" {
        return Ok(Parsed::Session {
            session,
            source: value
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    let turn = field(value, "turn_id")?;
    match event.as_str() {
        "UserPromptSubmit" => Ok(Parsed::Started {
            session,
            turn,
            prompt: field(value, "prompt")?,
        }),
        "Stop" => Ok(Parsed::Final {
            session,
            turn,
            text: optional_text(value, "last_assistant_message")?,
        }),
        "StopFailure" => Ok(Parsed::Failure {
            session,
            turn,
            message: "Provider turn failed; inspect its terminal.".into(),
            error: None,
        }),
        _ => Err(format!("Unsupported Bus hook event {event}")),
    }
}
