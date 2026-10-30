use super::*;

/// Captured from Grok 1.0.70 while a clean-first gate was still running.
/// The OSC title was the idle "grok" title, and the unsent Bus draft sat in
/// the composer. A live [stop] status line or a Tasks panel with work left
/// must outrank that idle title.
const BACKGROUND_TURN: &str =
    include_str!("../../../../tests/fixtures/grok-composer/native-1.0.70-background-turn.txt");

#[test]
fn captured_background_turn_is_working_despite_idle_osc() {
    assert!(
        BACKGROUND_TURN.contains("bus master (room 0):"),
        "fixture lost the unsent draft"
    );
    assert!(BACKGROUND_TURN.contains("[stop]"));
    assert!(BACKGROUND_TURN.contains("Tasks 1"));

    for (title, progress) in [("grok", "4;0;0"), ("", "")] {
        let result = detect_screen_with_osc(AgentKind::Grok, BACKGROUND_TURN, title, progress);
        assert_eq!(
            result.state,
            AgentState::Working,
            "idle OSC must not hide a live turn ({title:?} / {progress:?}): {result:?}"
        );
        assert!(result.visible_working, "{result:?}");
        assert!(!result.visible_idle, "{result:?}");
        let id = result.matched_rule.as_ref().map(|rule| rule.id.as_str());
        assert!(
            matches!(
                id,
                Some("spinner_status_working" | "background_tasks_panel_working")
            ),
            "{id:?}"
        );
    }
}

#[test]
fn tasks_panel_and_stop_line_outrank_idle_osc_but_zero_tasks_do_not() {
    let tasks = BACKGROUND_TURN
        .lines()
        .filter(|line| line.contains("Tasks") || line.contains('⸬'))
        .collect::<Vec<_>>()
        .join("\n");
    let panel = detect_screen_with_osc(AgentKind::Grok, &tasks, "grok", "4;0;0");
    assert_eq!(panel.state, AgentState::Working, "{panel:?}");
    assert!(panel.visible_working);
    assert_eq!(
        panel.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("background_tasks_panel_working")
    );

    let collapsed = "   ▸ Tasks 1\n";
    let collapsed = detect_screen_with_osc(AgentKind::Grok, collapsed, "grok", "4;0;0");
    assert_eq!(collapsed.state, AgentState::Working, "{collapsed:?}");

    let zero = detect_screen_with_osc(AgentKind::Grok, "   ▾ Tasks 0\n", "grok", "4;0;0");
    assert_eq!(zero.state, AgentState::Idle, "{zero:?}");
    assert_eq!(
        zero.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("osc_title_idle")
    );

    let stop = BACKGROUND_TURN
        .lines()
        .find(|line| line.contains("[stop]"))
        .expect("captured status line");
    let spinning = detect_screen_with_osc(AgentKind::Grok, stop, "grok", "4;0;0");
    assert_eq!(spinning.state, AgentState::Working, "{spinning:?}");
    assert!(spinning.visible_working);
    assert_eq!(
        spinning.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("spinner_status_working")
    );
}
