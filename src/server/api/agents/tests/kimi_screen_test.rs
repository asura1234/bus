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
