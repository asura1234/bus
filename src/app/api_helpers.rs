pub(super) fn tab_attention_priority(state: crate::detect::AgentState, seen: bool) -> u8 {
    match (state, seen) {
        (crate::detect::AgentState::Blocked, _) => 4,
        (crate::detect::AgentState::Idle, false) => 3,
        (crate::detect::AgentState::Working, _) => 2,
        (crate::detect::AgentState::Idle, true) => 1,
        (crate::detect::AgentState::Unknown, _) => 0,
    }
}

fn parse_api_key(key: &str) -> Option<crossterm::event::KeyEvent> {
    let normalized = normalize_api_key_alias(key.trim());
    let (code, modifiers) = crate::config::parse_key_combo(normalized)?;
    Some(crossterm::event::KeyEvent::new(code, modifiers))
}

fn normalize_api_key_alias(key: &str) -> &str {
    match key {
        "C-c" | "c-c" => "ctrl+c",
        "+" => "plus",
        _ => key,
    }
}

pub(super) fn encode_api_text(runtime: &crate::terminal::TerminalRuntime, text: &str) -> Vec<u8> {
    let bracketed = runtime.bracketed_paste_enabled();
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

pub(super) fn encode_api_keys(
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

pub(super) fn encode_api_submission_parts(
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

pub(super) fn encode_api_submission(
    runtime: &crate::terminal::TerminalRuntime,
    text: &str,
) -> Vec<u8> {
    let (mut text, enter) = encode_api_submission_parts(runtime, text);
    text.extend_from_slice(&enter);
    text
}

pub(super) fn encode_api_input(
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

pub(super) fn pane_agent_status(
    state: crate::detect::AgentState,
    seen: bool,
) -> crate::api::schema::AgentStatus {
    match (state, seen) {
        (crate::detect::AgentState::Idle, false) => crate::api::schema::AgentStatus::Done,
        (crate::detect::AgentState::Idle, true) => crate::api::schema::AgentStatus::Idle,
        (crate::detect::AgentState::Working, _) => crate::api::schema::AgentStatus::Working,
        (crate::detect::AgentState::Blocked, _) => crate::api::schema::AgentStatus::Blocked,
        (crate::detect::AgentState::Unknown, _) => crate::api::schema::AgentStatus::Unknown,
    }
}

pub(super) struct TerminalReadObservation {
    pub text: String,
    pub truncated: bool,
    pub viewport_rows: Option<u16>,
    pub viewport_columns: Option<u16>,
    pub requested_lines: Option<u32>,
    pub returned_lines: u32,
    pub available_lines: Option<u64>,
    pub exhausted: Option<bool>,
    pub revision: u64,
}

pub(super) fn read_terminal_snapshot(
    terminal: &crate::terminal::TerminalRuntime,
    source: crate::api::schema::ReadSource,
    format: crate::api::schema::ReadFormat,
    lines: Option<u32>,
) -> TerminalReadObservation {
    use crate::api::schema::{ReadFormat, ReadSource};

    let line_limit = lines.map(|lines| lines as usize);
    let recent_lines = line_limit.unwrap_or(80);
    let mut visible_facts = None;
    let snapshot = match (format, source) {
        (ReadFormat::Text, ReadSource::Visible) => {
            if line_limit.is_none() {
                if let Some((text, rows, columns, revision)) =
                    terminal.visible_text_snapshot_with_dimensions()
                {
                    visible_facts = Some((rows, columns, revision));
                    limit_snapshot_lines(text, None)
                } else {
                    limit_snapshot_lines(terminal.visible_text(), None)
                }
            } else {
                limit_snapshot_lines(terminal.visible_text(), line_limit)
            }
        }
        (ReadFormat::Text, ReadSource::Recent) => terminal.recent_text_snapshot(recent_lines),
        (ReadFormat::Text, ReadSource::RecentUnwrapped) => {
            terminal.recent_unwrapped_text_snapshot(recent_lines)
        }
        (ReadFormat::Text, ReadSource::Detection) => {
            limit_snapshot_lines(terminal.detection_text(), line_limit)
        }
        (ReadFormat::Ansi, ReadSource::Visible) => {
            limit_snapshot_lines(terminal.visible_ansi(), line_limit)
        }
        (ReadFormat::Ansi, ReadSource::Recent) => terminal.recent_ansi_snapshot(recent_lines),
        (ReadFormat::Ansi, ReadSource::RecentUnwrapped) => {
            terminal.recent_unwrapped_ansi_snapshot(recent_lines)
        }
        (ReadFormat::Ansi, ReadSource::Detection) => {
            limit_snapshot_lines(terminal.detection_text(), line_limit)
        }
    };
    let (rows, columns) = visible_facts
        .map(|(rows, columns, _)| (rows, columns))
        .unwrap_or_else(|| terminal.current_size());
    let rendered_rows = snapshot.text.split_inclusive('\n').count() as u32;
    let recent = matches!(source, ReadSource::Recent | ReadSource::RecentUnwrapped);
    let available_lines = recent.then(|| {
        terminal
            .scroll_metrics()
            .map_or(rendered_rows as u64, |metrics| {
                metrics
                    .max_offset_from_bottom
                    .saturating_add(metrics.viewport_rows) as u64
            })
    });
    let returned_lines = if recent {
        available_lines
            .unwrap_or_default()
            .min(lines.unwrap_or(80) as u64) as u32
    } else {
        lines.map_or(rendered_rows, |requested| requested.min(rendered_rows))
    };
    TerminalReadObservation {
        text: snapshot.text,
        truncated: snapshot.truncated,
        viewport_rows: (source == ReadSource::Visible).then_some(rows),
        viewport_columns: (source == ReadSource::Visible).then_some(columns),
        requested_lines: lines,
        returned_lines,
        available_lines,
        exhausted: recent.then_some(!snapshot.truncated),
        revision: visible_facts
            .map(|(_, _, revision)| revision)
            .unwrap_or_else(|| terminal.content_seq()),
    }
}

pub(crate) fn limit_snapshot_lines(
    text: String,
    limit: Option<usize>,
) -> crate::pane::TerminalReadSnapshot {
    let Some(limit) = limit else {
        return crate::pane::TerminalReadSnapshot {
            text,
            truncated: false,
        };
    };
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    crate::pane::TerminalReadSnapshot {
        text: lines[lines.len().saturating_sub(limit)..].concat(),
        truncated: lines.len() > limit,
    }
}

pub(super) fn normalize_reported_agent_label(agent: &str) -> Option<String> {
    let trimmed = agent.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(agent) = crate::detect::parse_agent_label(trimmed) {
        return Some(crate::detect::agent_label(agent).to_string());
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod read_snapshot_tests {
    use super::limit_snapshot_lines;

    #[test]
    fn line_limit_preserves_endings_and_reports_omitted_lines() {
        let snapshot = limit_snapshot_lines("one\ntwø\n三\n".into(), Some(2));
        assert_eq!(snapshot.text, "twø\n三\n");
        assert!(snapshot.truncated);

        let snapshot = limit_snapshot_lines("one\ntwo\nthree".into(), Some(1));
        assert_eq!(snapshot.text, "three");
        assert!(snapshot.truncated);

        let snapshot = limit_snapshot_lines("one\ntwo".into(), Some(0));
        assert_eq!(snapshot.text, "");
        assert!(snapshot.truncated);

        let snapshot = limit_snapshot_lines("".into(), Some(2));
        assert_eq!(snapshot.text, "");
        assert!(!snapshot.truncated);
    }

    #[test]
    fn omitted_line_limit_returns_the_complete_snapshot() {
        let snapshot = limit_snapshot_lines("one\ntwo\n".into(), None);
        assert_eq!(snapshot.text, "one\ntwo\n");
        assert!(!snapshot.truncated);
    }

    #[tokio::test]
    async fn recent_read_honors_line_requests_above_one_thousand() {
        let terminal =
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(80, 3, 10_000_000, &[]);
        for index in 0..1500 {
            terminal.test_process_pty_bytes(format!("{index:06}\r\n").as_bytes());
        }
        let snapshot = super::read_terminal_snapshot(
            &terminal,
            crate::api::schema::ReadSource::Recent,
            crate::api::schema::ReadFormat::Text,
            Some(5000),
        );
        let returned = snapshot
            .text
            .split_inclusive('\n')
            .filter(|line| !line.is_empty())
            .count();
        assert!(
            returned > 1000,
            "expected more than the old 1000-line clamp, got {returned}"
        );
        assert!(
            snapshot.text.contains("000000"),
            "honored 5000-line window should include the oldest retained row"
        );
        assert!(
            !snapshot.truncated,
            "fewer rows than requested means available history is exhausted"
        );
    }

    #[tokio::test]
    async fn room_orchestrator_core_recent_read_reports_exact_range_facts() {
        let terminal =
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(12, 4, 1_000_000, &[]);
        for index in 0..30 {
            terminal.test_process_pty_bytes(format!("row-{index:02}\r\n").as_bytes());
        }
        let snapshot = super::read_terminal_snapshot(
            &terminal,
            crate::api::schema::ReadSource::Recent,
            crate::api::schema::ReadFormat::Text,
            Some(17),
        );
        assert_eq!(snapshot.requested_lines, Some(17));
        assert_eq!(snapshot.returned_lines, 17);
        assert!(snapshot
            .available_lines
            .is_some_and(|available| available >= 17));
        assert_eq!(snapshot.exhausted, Some(false));
        assert!(snapshot.revision > 0);
    }

    #[tokio::test]
    async fn room_orchestrator_core_visible_read_reports_complete_viewport_facts() {
        let terminal = crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
            12, 4, 1_000_000, b"a\r\nb",
        );
        let snapshot = super::read_terminal_snapshot(
            &terminal,
            crate::api::schema::ReadSource::Visible,
            crate::api::schema::ReadFormat::Text,
            None,
        );
        assert_eq!(snapshot.viewport_rows, Some(4));
        assert_eq!(snapshot.viewport_columns, Some(12));
        assert_eq!(snapshot.requested_lines, None);
        assert_eq!(snapshot.available_lines, None);
        assert_eq!(snapshot.exhausted, None);
        assert!(!snapshot.truncated);
    }
}
