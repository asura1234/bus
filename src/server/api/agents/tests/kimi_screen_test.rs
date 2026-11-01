use super::*;
use crate::terminal::runtime::SubmissionInspector;

const COMMAND: &str = include_str!("../../../../../tests/fixtures/kimi-steer/command.txt");
const THINKING: &str = include_str!("../../../../../tests/fixtures/kimi-steer/thinking.txt");
const APPROVAL: &str = include_str!("../../../../../tests/fixtures/kimi-steer/approval.txt");

#[test]
fn kimi_captured_thinking_and_command_composers_accept_steering() {
    for screen in [THINKING, COMMAND] {
        assert_eq!(
            observe(AgentKind::Kimi, screen, "from bus"),
            InputObservation::Empty
        );
    }
}

#[test]
fn kimi_captured_approval_never_authorizes_composer_input() {
    assert_eq!(
        observe(AgentKind::Kimi, APPROVAL, "from bus"),
        InputObservation::Unavailable
    );
    // A stale framed composer above a live approval must not win either.
    assert_eq!(
        observe(
            AgentKind::Kimi,
            &format!("{COMMAND}\n{APPROVAL}"),
            "from bus"
        ),
        InputObservation::Unavailable
    );
}

#[test]
fn kimi_editor_ignores_working_chrome_but_requires_the_same_composer() {
    let inspector = preserving_observer(
        AgentKind::Kimi,
        "from bus".into(),
        std::path::PathBuf::new(),
        vec![],
    );
    let repaint = COMMAND.replace('🌒', "🌕").replace("sleep 45", "sleep 44");
    assert_ne!(repaint, COMMAND);
    assert!(inspector.editor_frame_unchanged(COMMAND, &repaint));
    assert!(!inspector.editor_frame_unchanged(COMMAND, APPROVAL));
    assert!(!inspector.editor_frame_unchanged(COMMAND, &repaint.replace("│ > ", "│ > edited")));
}

#[test]
fn kimi_busy_composer_with_a_draft_is_not_submission_acceptance() {
    let inspector = preserving_observer(
        AgentKind::Kimi,
        "from bus".into(),
        std::path::PathBuf::new(),
        vec![],
    );
    assert!(!inspector.editor_working(&COMMAND.replace("│ > ", "│ > from bus")));
}

#[test]
fn kimi_idle_long_prompt_transfer_allows_transcript_reflow_but_not_draft_edits() {
    let idle = include_str!("../../../../../tests/fixtures/kimi-steer/idle.txt");
    let lines: Vec<_> = idle.lines().collect();
    let top = lines
        .iter()
        .rposition(|line| line.trim().starts_with('╭'))
        .unwrap();
    let bottom = lines
        .iter()
        .rposition(|line| line.trim().starts_with('╰'))
        .unwrap();
    let draft: Vec<_> = (0..24)
        .map(|n| format!("review request line {n:02}: keep this exact text"))
        .collect();
    let text = draft.join("\n");
    assert!(text.len() > 1000, "exercise a long idle review request");
    let mut composer = vec![lines[top].to_owned()];
    composer.extend(
        draft
            .iter()
            .enumerate()
            .map(|(n, row)| format!(" │ {}{row:<49}│", if n == 0 { "> " } else { "  " })),
    );
    composer.extend(lines[bottom..].iter().map(|line| (*line).to_owned()));
    let composer = composer.join("\n");
    let before = format!(
        "{}\n{composer}",
        lines[top.saturating_sub(10)..top].join("\n")
    );
    // Growing the native composer scrolls/repaints transcript rows above it.
    // Model the frame guard after native-editor verification of the full draft.
    let after = format!(
        "{}\n{composer}",
        lines[top.saturating_sub(7)..top].join("\n")
    );
    let inspector = preserving_observer(AgentKind::Kimi, text, std::path::PathBuf::new(), vec![]);
    assert_eq!(inspector.observe(&before), InputObservation::Pending);
    assert_eq!(inspector.observe(&after), InputObservation::Pending);
    assert_ne!(before, after);
    assert!(
        inspector.editor_frame_unchanged(&before, &after),
        "benign reflow must not strand the verified long draft before Enter"
    );
    assert!(!inspector.editor_frame_unchanged(
        &before,
        &after.replace("review request line 23", "foreign draft line 23")
    ));
    assert!(!inspector.editor_frame_unchanged(&before, APPROVAL));
}
