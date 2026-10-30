use super::*;

/// Captured from Grok 1.0.70 while a clean-first gate was still running.
/// The OSC title was the idle "grok" title, and the unsent Bus draft sat in
/// the composer. A live [stop] status line or a running task row must
/// outrank that idle title. The Tasks count includes completed rows.
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

    // The header count stays after every row is finished. It is not work left.
    let collapsed = "   ▸ Tasks 1\n";
    let collapsed = detect_screen_with_osc(AgentKind::Grok, collapsed, "grok", "4;0;0");
    assert_eq!(collapsed.state, AgentState::Idle, "{collapsed:?}");
    assert_eq!(
        collapsed.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("osc_title_idle")
    );

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

/// Grok 1.0.70 parks a prompt submitted during a turn under Tasks as
/// "▾ Queued N". That queue is accepted work waiting to run. An idle OSC
/// title must not hide it, or Bus closes the request as reply_not_captured
/// before the queued turn starts. Queued 0 and a transcript sentence are not
/// that panel.
#[test]
fn queued_prompts_outrank_idle_osc_and_zero_does_not() {
    for screen in ["   ▾ Queued 1\n", "   ▸ Queued 12\n"] {
        let result = detect_screen_with_osc(AgentKind::Grok, screen, "grok", "4;0;0");
        assert_eq!(result.state, AgentState::Working, "{screen:?} {result:?}");
        assert!(result.visible_working, "{result:?}");
        assert!(!result.visible_idle, "{result:?}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("queued_prompts_working"),
            "{screen:?}"
        );
    }

    let zero = detect_screen_with_osc(AgentKind::Grok, "   ▾ Queued 0\n", "grok", "4;0;0");
    assert_eq!(zero.state, AgentState::Idle, "{zero:?}");
    assert_eq!(
        zero.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("osc_title_idle")
    );

    let prose = detect_screen_with_osc(
        AgentKind::Grok,
        "Bus closed it because Queued 1 was still waiting\n",
        "grok",
        "4;0;0",
    );
    assert_eq!(prose.state, AgentState::Idle, "{prose:?}");
    assert_ne!(
        prose.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("queued_prompts_working")
    );
}

#[test]
fn still_running_background_status_is_working_despite_idle_osc() {
    // Grok 1.0.70 draws these above the prompt while background work continues
    // and the turn itself looks idle, including after the [stop] line is gone.
    let lines = [
        "◎ 1 command · 2 monitors · 1 loop · 1 subagent still running\n",
        "◎ 1 command still running · send a message to interrupt\n",
        "◎ waiting · send a message to interrupt\n",
    ];
    for line in lines {
        let result = detect_screen_with_osc(AgentKind::Grok, line, "grok", "4;0;0");
        assert_eq!(
            result.state,
            AgentState::Working,
            "background work must not count as idle: {line:?} -> {result:?}"
        );
        assert!(result.visible_working, "{line:?} -> {result:?}");
        assert!(!result.visible_idle, "{line:?} -> {result:?}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("still_running_background_status_working"),
            "{line:?}"
        );
    }

    let zero = detect_screen_with_osc(
        AgentKind::Grok,
        "◎ 0 commands still running\n",
        "grok",
        "4;0;0",
    );
    assert_eq!(zero.state, AgentState::Idle, "{zero:?}");
    assert_eq!(
        zero.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("osc_title_idle")
    );
}

#[test]
fn watchers_and_native_queue_outrank_idle_osc() {
    for (screen, rule) in [
        ("Watchers 1\n", "watchers_working"),
        ("▾ Watchers 2\n", "watchers_working"),
        ("▸ Watchers 12\n", "watchers_working"),
        ("Queued 1\n", "queued_prompts_working"),
        (
            "1 command still running\n",
            "still_running_background_status_working",
        ),
        ("1 queued\n", "native_queue_hint_working"),
        ("queued, Enter to send now\n", "native_queue_hint_working"),
    ] {
        let result = detect_screen_with_osc(AgentKind::Grok, screen, "grok", "4;0;0");
        assert_eq!(result.state, AgentState::Working, "{screen:?} {result:?}");
        assert!(result.visible_working, "{screen:?} {result:?}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some(rule),
            "{screen:?}"
        );
    }

    for screen in [
        "Watchers 0\n",
        "Watchers\n",
        "Reason Paused Watchers Status\n",
        "0 queued\n",
        "the draft was queued yesterday\n",
        "1 command still running in the transcript\n",
    ] {
        let result = detect_screen_with_osc(AgentKind::Grok, screen, "grok", "4;0;0");
        assert_eq!(result.state, AgentState::Idle, "{screen:?} {result:?}");
        assert!(!result.visible_working, "{screen:?} {result:?}");
    }
}

#[test]
fn cancel_footer_without_stop_line_outranks_idle_osc() {
    let thinking = "⠹ Thinking…\nEsc:cancel │ Ctrl+.:shortcuts\n";
    let result = detect_screen_with_osc(AgentKind::Grok, thinking, "grok", "4;0;0");
    assert_eq!(result.state, AgentState::Working, "{result:?}");
    assert!(result.visible_working, "{result:?}");
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("esc_cancel_hints_working")
    );

    let bare = detect_screen_with_osc(AgentKind::Grok, "⠹ Thinking…\n", "grok", "4;0;0");
    assert_eq!(bare.state, AgentState::Idle, "{bare:?}");
    assert_eq!(
        bare.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("osc_title_idle")
    );
    let hint_only = detect_screen_with_osc(AgentKind::Grok, "Esc:cancel\n", "grok", "4;0;0");
    assert_eq!(hint_only.state, AgentState::Idle, "{hint_only:?}");
}

#[test]
fn completed_tasks_panel_does_not_hold_delivery() {
    let done = "▾ Tasks 1\n✓ cargo build\n";
    let result = detect_screen_with_osc(AgentKind::Grok, done, "grok", "4;0;0");
    assert_eq!(result.state, AgentState::Idle, "{result:?}");
    assert!(!result.visible_working, "{result:?}");
    assert_ne!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("background_tasks_panel_working")
    );

    let mixed = "▾ Tasks 2\n✓ cargo build\n⸬ cargo test\n";
    let mixed = detect_screen_with_osc(AgentKind::Grok, mixed, "grok", "4;0;0");
    assert_eq!(mixed.state, AgentState::Working, "{mixed:?}");
    assert!(mixed.visible_working);
    assert_eq!(
        mixed.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("background_tasks_panel_working")
    );

    let zero = detect_screen_with_osc(AgentKind::Grok, "▾ Tasks 0\n", "grok", "4;0;0");
    assert_eq!(zero.state, AgentState::Idle, "{zero:?}");
}

/// Grok 1.0.70 foreground shell, captured while one Ctrl+C was about to be
/// sent. The status line is a braille spinner plus `[stop]`, and the footer
/// is `Ctrl+c:cancel` with `Ctrl+b:send to bg`.
const FOREGROUND_STOP: &str =
    include_str!("../../../../tests/fixtures/grok-composer/native-1.0.70-foreground-stop.txt");

/// The same turn on the next sample. The transcript says the user cancelled
/// it, `[stop]` is gone, and the footer no longer offers `Ctrl+c:cancel`.
const FOREGROUND_CANCELLED: &str =
    include_str!("../../../../tests/fixtures/grok-composer/native-1.0.70-foreground-cancelled.txt");

/// `Waiting for response… [stop]` with footer `Ctrl+c:cancel`, captured at
/// 0.6s. One Ctrl+C unsends that submission.
const WAITING_STOP: &str =
    include_str!("../../../../tests/fixtures/grok-composer/native-1.0.70-waiting-stop.txt");

/// The sample after that Ctrl+C. `[stop]` is gone and the prompt is back in
/// the composer with `Enter:send`.
const WAITING_UNSENT: &str =
    include_str!("../../../../tests/fixtures/grok-composer/native-1.0.70-waiting-unsent.txt");

/// Second Ctrl+C on a foreground shell. It arms quit. It is not a working turn.
const PRESS_AGAIN: &str =
    include_str!("../../../../tests/fixtures/grok-composer/native-1.0.70-press-again.txt");

#[test]
fn captured_foreground_cancel_clears_stop_and_stays_idle() {
    assert!(FOREGROUND_STOP.contains("[stop]"));
    assert!(FOREGROUND_STOP.contains("Ctrl+c:cancel"));
    assert!(FOREGROUND_STOP.contains("Ctrl+b:send to bg"));
    assert!(FOREGROUND_STOP.contains("Wait 150 seconds in the foreground"));
    let working = detect_screen_with_osc(AgentKind::Grok, FOREGROUND_STOP, "grok", "4;0;0");
    assert_eq!(working.state, AgentState::Working, "{working:?}");
    assert!(working.visible_working, "{working:?}");
    assert!(!working.visible_idle, "{working:?}");
    assert_eq!(
        working.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("spinner_status_working"),
        "{working:?}"
    );

    assert!(FOREGROUND_CANCELLED.contains("Turn cancelled by user"));
    assert!(!FOREGROUND_CANCELLED.contains("[stop]"));
    assert!(!FOREGROUND_CANCELLED.contains("Ctrl+c:cancel"));
    let idle = detect_screen_with_osc(AgentKind::Grok, FOREGROUND_CANCELLED, "grok", "4;0;0");
    assert_eq!(idle.state, AgentState::Idle, "{idle:?}");
    assert!(!idle.visible_working, "{idle:?}");
    assert_eq!(
        idle.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("osc_title_idle"),
        "{idle:?}"
    );

    assert!(WAITING_STOP.contains("Waiting for response"));
    assert!(WAITING_STOP.contains("[stop]"));
    assert!(WAITING_STOP.contains("Ctrl+c:cancel"));
    assert!(!WAITING_STOP.contains("Ctrl+b:send to bg"));
    let waiting = detect_screen_with_osc(AgentKind::Grok, WAITING_STOP, "grok", "4;0;0");
    assert_eq!(waiting.state, AgentState::Working, "{waiting:?}");
    assert_eq!(
        waiting.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("spinner_status_working"),
        "{waiting:?}"
    );

    assert!(WAITING_UNSENT.contains("Enter:send"));
    assert!(!WAITING_UNSENT.contains("[stop]"));
    let unsent = detect_screen_with_osc(AgentKind::Grok, WAITING_UNSENT, "grok", "4;0;0");
    assert_eq!(unsent.state, AgentState::Idle, "{unsent:?}");
    assert!(!unsent.visible_working, "{unsent:?}");

    assert!(PRESS_AGAIN.contains("Ctrl+c:press again to quit"));
    assert!(!PRESS_AGAIN.contains("[stop]"));
    let quit = detect_screen_with_osc(AgentKind::Grok, PRESS_AGAIN, "grok", "4;0;0");
    assert_eq!(quit.state, AgentState::Idle, "{quit:?}");
    assert!(!quit.visible_working, "{quit:?}");
}
