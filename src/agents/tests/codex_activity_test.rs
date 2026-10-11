use super::manifest::{detect_with_osc, DetectionInput};
use super::{AgentDetection, AgentKind, AgentState};

fn codex_detection(screen: &str, osc_title: &str) -> AgentDetection {
    detect_with_osc(
        AgentKind::Codex,
        DetectionInput {
            screen,
            osc_title,
            osc_progress: "",
        },
    )
}

const STALE_QUESTION: &str = concat!(
    "• Waiting for background terminal (39m 17s • esc to interrupt) · 1 background terminal\n",
    "  └ python3 skills/gate-and-fix/scripts/gate_and_fix.py\n\n",
    "• Queued follow-up inputs\n",
    "  ? 1 question\n",
    "    shift+← to answer\n\n",
    "› Ask Codex to do anything\n",
);

#[test]
fn working_codex_with_a_stale_question_banner_is_not_blocked() {
    for (activity, hint) in [
        (
            "• Working (12s • esc to interrupt)",
            "    shift+left to answer",
        ),
        (
            "• Waiting for background terminal (39m 17s • esc to interrupt) · 1 background terminal",
            "    shift+← to answer",
        ),
    ] {
        let screen = format!(
            "{activity}\n\n• Queued follow-up inputs\n  ? 1 question\n{hint}\n\n› Ask Codex to do anything\n"
        );
        let result = codex_detection(&screen, "[ ! ] Action Required | bus");
        assert_eq!(result.state, AgentState::Working, "{screen}");
        assert!(result.visible_working, "{screen}");
        assert!(!result.visible_blocker, "{screen}");
    }
}

#[test]
fn live_screen_waiting_on_a_background_terminal_stays_working() {
    let result = codex_detection(STALE_QUESTION, "[ . ] Action Required | bus");
    assert_eq!(result.state, AgentState::Working);
    assert!(!result.visible_blocker);
}

#[test]
fn codex_background_activity_with_a_collapsed_question_remains_working() {
    // Reported live on codex-dev while the server still bundled 2026.09.13.3.
    let screen = concat!(
        "• Working (26m 19s • esc to interrupt) · 1 background terminal running · /ps to view · /stop to close\n\n",
        "• Queued follow-up inputs\n",
        "  ? 1 question\n",
        "    shift+left to answer\n\n",
        "› Ask Codex to do anything\n",
    );
    for screen in [
        screen.to_owned(),
        screen.replace("/stop to close", "/stop\n  to close"),
    ] {
        let result = codex_detection(&screen, "[ ! ] Action Required | bus");
        assert_eq!(result.state, AgentState::Working, "{screen}");
        assert!(result.visible_working, "{screen}");
        assert!(!result.visible_blocker, "{screen}");
        assert!(crate::agents::dialog::parse(&screen).is_none(), "{screen}");
    }
}

#[test]
fn a_live_permission_dialog_still_blocks_beside_the_stale_banner() {
    let screen = concat!(
        "• Working (4s • esc to interrupt)\n",
        "• Queued follow-up inputs\n",
        "  ? 1 question\n",
        "    shift+left to answer\n",
        "Press enter to confirm or esc to cancel\n",
        "› 1. Yes, proceed\n",
    );
    let result = codex_detection(screen, "[ ! ] Action Required | bus");
    assert_eq!(result.state, AgentState::Blocked);
    assert!(result.visible_blocker);
}

#[test]
fn expanded_codex_text_question_blocks_while_collapsed_questions_stay_working() {
    for footer in [
        "enter submit   ctrl+] skip   shift+→ main prompt",
        "enter submit   ⌃] skip   ⇧→ main prompt",
    ] {
        let screen = format!("• Working (17s • esc to interrupt)\n• Queued follow-up inputs\nWhat token should Bus use?\nType your answer\n{footer}\n");
        let result = codex_detection(&screen, "[ ! ] Action Required | bus");
        assert_eq!(result.state, AgentState::Blocked, "{screen}");
        assert!(result.visible_blocker);
    }
}

#[test]
fn codex_running_its_own_updater_is_not_idle() {
    let updating = concat!(
        "› Ask Codex to do anything\n\n",
        "✗ codex resume 01a11654 --no-daemon\n\n",
        "Updating Codex via `npm install -g @openai/codex`...\n",
        "⠙\n",
    );
    // The stale Codex title must not read as idle either. Working, not
    // Unknown: an install can outlast the queued-message stall timeout, and
    // Bus waits on a working agent instead of closing its queue.
    let result = codex_detection(updating, "bus");
    assert_eq!(result.state, AgentState::Working);
    assert!(!result.skip_state_update, "the change must be published");

    // Once Codex is back, its prompt sits below the old update line.
    let relaunched = format!("{updating}\n› Ask Codex to do anything\n\n  ? for shortcuts\n");
    assert_eq!(codex_detection(&relaunched, "bus").state, AgentState::Idle);
}
