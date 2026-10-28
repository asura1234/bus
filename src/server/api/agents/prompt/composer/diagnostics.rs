//! One content-free explanation at the terminal refusal, never in polling.
use super::{composer_frame, image_chips, input, styled_chars, AgentKind};

#[derive(Default)]
pub(super) struct History {
    pub tracked_body: bool,
    pub lost: bool,
    pub previous_body_bytes: usize,
    pub prior_body_match: bool,
    pub remainder_match: bool,
    pub remainder_count: usize,
    pub modeled_text_bytes: usize,
    pub modeled_cursor: usize,
}

#[derive(Default)]
pub(super) struct Policy {
    pub keys_available: bool,
    pub native_stash: bool,
    pub invisible_content: bool,
    pub opaque_draft: bool,
}

pub(super) fn refusal(
    kind: AgentKind,
    screen: &str,
    expected: &str,
    phase: &'static str,
    cursor: Option<(u16, u16)>,
    history: Option<History>,
    policy: Option<Policy>,
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
    let detection = crate::agents::manifest::detect_with_osc(
        kind,
        crate::agents::manifest::DetectionInput {
            screen: &plain,
            osc_title: "",
            osc_progress: "",
        },
    );
    let body = input(kind, screen);
    let text = body.as_deref().unwrap_or_default();
    let normalize = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    let normalized_body = normalize(text);
    let normalized_expected = normalize(expected);
    let first_difference = normalized_body
        .chars()
        .zip(normalized_expected.chars())
        .position(|(a, b)| a != b);
    let normalized_match = normalized_body == normalized_expected;
    let collapsed_match = super::collapsed_paste(kind, text, expected);
    let image_match = kind == AgentKind::Claude && super::lifted_images(text, expected, normalize)
        || kind == AgentKind::Codex && super::codex_image_attachment(text, expected);
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
    let collapsed_lines = text
        .split_once(" +")
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
    let history_available = history.is_some();
    let history = history.unwrap_or_default();
    let policy_available = policy.is_some();
    let policy = policy.unwrap_or_default();
    tracing::debug!(target: "bus::terminal::submission", event = "bus.composer.ownership",
        provider = crate::agents::agent_label(kind), phase, cursor = ?cursor,
        pane_state = ?detection.state, visible_idle = detection.visible_idle,
        visible_working = detection.visible_working, visible_blocker = detection.visible_blocker,
        screen_rows = rows.len(), prompt_row = ?prompt_row, faint_chars,
        composer_frame = composer_frame(kind, &plain), body_present = body.is_some(),
        body_empty = body.as_deref().is_some_and(str::is_empty), body_bytes = text.len(),
        body_chars = text.chars().count(), body_lines = text.lines().count(), expected_bytes = expected.len(),
        expected_chars = expected.chars().count(), expected_newlines = expected.replace("\r\n", "\n").replace('\r', "\n").matches('\n').count(),
        normalized_body_chars = normalized_body.chars().count(), normalized_expected_chars = normalized_expected.chars().count(),
        first_difference = ?first_difference, normalized_match, collapsed_match, collapsed_lines = ?collapsed_lines,
        image_match, expected_images, claude_image_chips = image_chips(text).0,
        history_available, tracked_body = history.tracked_body, ownership_lost = history.lost,
        previous_body_bytes = history.previous_body_bytes, prior_body_match = history.prior_body_match,
        remainder_match = history.remainder_match, remainder_count = history.remainder_count,
        modeled_text_bytes = history.modeled_text_bytes, modeled_cursor = history.modeled_cursor,
        policy_available, preservation_keys_available = policy.keys_available,
        native_stash_present = policy.native_stash, invisible_content = policy.invisible_content,
        opaque_draft = policy.opaque_draft,
        "Native submission refused; content-free composer ownership evidence");
}
