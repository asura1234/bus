//! Explain ownership refusals without recording text, paths or text hashes.
use super::{composer_frame, image_chips, styled_chars, AgentKind};
use crate::agents::AgentDetection;

pub(super) fn refusal(
    kind: AgentKind,
    screen: &str,
    expected: &str,
    body: Option<&str>,
    detection: AgentDetection,
    checks: [bool; 3],
    reason: &'static str,
) {
    if !tracing::enabled!(target: "bus::terminal::submission", tracing::Level::DEBUG) {
        return;
    }
    let rows: Vec<_> = screen.lines().map(styled_chars).collect();
    let faint_chars = rows.iter().flatten().filter(|c| c.faint).count();
    let plain = rows
        .iter()
        .map(|row| row.iter().map(|c| c.character).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    let normalized = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    let normalized_body = normalized(body.unwrap_or_default());
    let normalized_expected = normalized(expected);
    let first_difference = normalized_body
        .chars()
        .zip(normalized_expected.chars())
        .position(|(a, b)| a != b);
    let marker = match kind {
        AgentKind::Claude => '❯',
        AgentKind::Codex => '›',
        _ => '>',
    };
    let prompt_row = plain
        .lines()
        .enumerate()
        .filter_map(|(index, line)| line.trim_start().starts_with(marker).then_some(index))
        .last();
    let collapsed_lines = body
        .and_then(|body| body.split_once(" +"))
        .and_then(|(_, count)| count.strip_suffix(" lines]"))
        .and_then(|n| n.parse::<usize>().ok());
    let expected_images = match kind {
        AgentKind::Claude => {
            crate::agents::providers::claude_code::hooks::claude_lifted_images(expected).0
        }
        AgentKind::Codex => crate::agents::providers::codex::hooks::image_paste_parts(expected)
            .0
            .len(),
        _ => 0,
    };
    tracing::debug!(target: "bus::terminal::submission", event = "bus.composer.ownership",
        provider = crate::agents::agent_label(kind), reason,
        pane_state = ?detection.state, visible_idle = detection.visible_idle,
        visible_working = detection.visible_working, visible_blocker = detection.visible_blocker,
        screen_rows = rows.len(), prompt_row = ?prompt_row, faint_chars,
        composer_frame = composer_frame(kind, &plain), body_present = body.is_some(),
        body_empty = body.is_some_and(str::is_empty), body_bytes = body.map_or(0, str::len),
        body_chars = body.map_or(0, |body| body.chars().count()),
        body_lines = body.map_or(0, |body| body.lines().count()), expected_bytes = expected.len(),
        expected_chars = expected.chars().count(), expected_newlines = expected.replace("\r\n", "\n").replace('\r', "\n").matches('\n').count(),
        normalized_body_chars = normalized_body.chars().count(), normalized_expected_chars = normalized_expected.chars().count(),
        first_difference = ?first_difference, normalized_match = checks[0], collapsed_match = checks[1],
        collapsed_lines = ?collapsed_lines, image_match = checks[2], expected_images,
        claude_image_chips = image_chips(body.unwrap_or_default()).0,
        "Composer ownership check did not recognize the submitted draft");
}
