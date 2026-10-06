use super::manifest::{explain_with_input, DetectionInput};
use super::{Agent, AgentState};

fn explain(screen: &str, osc_title: &str) -> super::manifest::DetectionExplain {
    explain_with_input(
        Agent::Codex,
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
        let result = explain(&screen, "[ ! ] Action Required | bus");
        assert_eq!(result.state, AgentState::Working, "{screen}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("activity_with_stale_question"),
            "{screen}"
        );
        assert!(result.visible_working, "{screen}");
        assert!(!result.visible_blocker, "{screen}");
    }
}

#[test]
fn live_screen_waiting_on_a_background_terminal_stays_working() {
    let result = explain(STALE_QUESTION, "[ . ] Action Required | bus");
    assert_eq!(result.state, AgentState::Working);
    assert!(!result.visible_blocker);
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
    let result = explain(screen, "[ ! ] Action Required | bus");
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
        let result = explain(&screen, "[ ! ] Action Required | bus");
        assert_eq!(result.state, AgentState::Blocked, "{screen}");
        assert!(result.visible_blocker);
        assert_eq!(result.matched_rule.unwrap().id, "expanded_text_question");
    }
}
