pub(in crate::server) fn tab_attention_priority(
    state: crate::agents::AgentState,
    seen: bool,
) -> u8 {
    match (state, seen) {
        (crate::agents::AgentState::Blocked, _) => 4,
        (crate::agents::AgentState::Idle, false) => 3,
        (crate::agents::AgentState::Working, _) => 2,
        (crate::agents::AgentState::Idle, true) => 1,
        (crate::agents::AgentState::Unknown, _) => 0,
    }
}

fn parse_api_key(key: &str) -> Option<crossterm::event::KeyEvent> {
    let normalized = normalize_api_key_alias(key.trim());
    let (code, modifiers) = crate::utils::config::parse_key_combo(normalized)?;
    Some(crossterm::event::KeyEvent::new(code, modifiers))
}

fn normalize_api_key_alias(key: &str) -> &str {
    match key {
        "C-c" | "c-c" => "ctrl+c",
        "+" => "plus",
        _ => key,
    }
}

pub(in crate::server) fn encode_api_text(
    runtime: &crate::terminal::TerminalRuntime,
    text: &str,
) -> Vec<u8> {
    let bracketed = runtime.bracketed_paste_enabled();
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

pub(in crate::server) fn encode_api_keys(
    runtime: &crate::terminal::TerminalRuntime,
    keys: &[String],
) -> Result<Vec<Vec<u8>>, String> {
    let mut encoded_keys = Vec::with_capacity(keys.len());
    for key in keys {
        let Some(key_event) = parse_api_key(key) else {
            return Err(key.clone());
        };
        encoded_keys.push(runtime.encode_terminal_key(key_event.into()));
    }
    Ok(encoded_keys)
}

pub(in crate::server) fn encode_api_submission_parts(
    runtime: &crate::terminal::TerminalRuntime,
    text: &str,
) -> (Vec<u8>, Vec<u8>) {
    let text = encode_api_text(runtime, text);
    let enter = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    );
    (text, runtime.encode_terminal_key(enter.into()))
}

pub(in crate::server) fn encode_api_submission(
    runtime: &crate::terminal::TerminalRuntime,
    text: &str,
) -> Vec<u8> {
    let (mut text, enter) = encode_api_submission_parts(runtime, text);
    text.extend_from_slice(&enter);
    text
}

pub(in crate::server) fn encode_api_input(
    runtime: &crate::terminal::TerminalRuntime,
    text: &str,
    keys: &[String],
) -> Result<Vec<u8>, String> {
    let mut bytes = if text.is_empty() {
        Vec::new()
    } else {
        encode_api_text(runtime, text)
    };
    for encoded in encode_api_keys(runtime, keys)? {
        bytes.extend_from_slice(&encoded);
    }
    Ok(bytes)
}

pub(in crate::server) fn pane_agent_status(
    state: crate::agents::AgentState,
    seen: bool,
) -> crate::protocol::api::schema::AgentStatus {
    match (state, seen) {
        (crate::agents::AgentState::Idle, false) => crate::protocol::api::schema::AgentStatus::Done,
        (crate::agents::AgentState::Idle, true) => crate::protocol::api::schema::AgentStatus::Idle,
        (crate::agents::AgentState::Working, _) => {
            crate::protocol::api::schema::AgentStatus::Working
        }
        (crate::agents::AgentState::Blocked, _) => {
            crate::protocol::api::schema::AgentStatus::Blocked
        }
        (crate::agents::AgentState::Unknown, _) => {
            crate::protocol::api::schema::AgentStatus::Unknown
        }
    }
}

pub(crate) use super::terminal_read::limit_snapshot_lines;
pub(in crate::server) use super::terminal_read::read_terminal_snapshot;

pub(in crate::server) fn normalize_reported_agent_label(agent: &str) -> Option<String> {
    let trimmed = agent.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(agent) = crate::agents::parse_agent_label(trimmed) {
        return Some(crate::agents::agent_label(agent).to_string());
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
#[path = "tests/terminal_read_test.rs"]
mod read_snapshot_tests;
