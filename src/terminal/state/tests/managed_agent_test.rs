#[test]
fn managed_agent_becomes_ready_when_detection_is_idle() {
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.begin_managed_agent(
        "builder".into(),
        AgentKind::Claude,
        now,
        Duration::from_millis(1),
        Duration::from_secs(30),
    );
    terminal.set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
    assert!(terminal.reconcile_managed_agent_at(now + Duration::from_millis(2), false));
    assert!(terminal.managed_agent_interactive_ready());
}

#[test]
fn staffing_codex_captured_startup_screen_confirms_native_readiness_without_a_session() {
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.begin_managed_agent(
        "bus-r8259-a21848".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(3),
        Duration::from_secs(30),
    );
    let screen = "› Ask Codex to do anything\n  GPT-6.1-Sol xhigh · ~/work/libtv-app-prd-canvasbar\n  ? for shortcuts                       ⚠ 1 warning · f2 to view\n";
    let detection = crate::agents::detect_agent_with_launch_context(
        Some(AgentKind::Codex),
        screen,
        "⠋ Codex",
        "",
        terminal.managed_agent_launch_pending(),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), detection.state);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(4), false);
    assert!(terminal.managed_agent_interactive_ready());
    assert!(terminal.persisted_agent_session.is_none());
}

#[test]
fn promptless_codex_keeps_its_startup_watch_until_the_deadline() {
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.begin_managed_agent(
        "promptless".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(3),
        Duration::from_secs(30),
    );
    let detection = crate::agents::detect_agent_with_launch_context(
        Some(AgentKind::Codex),
        "BUS_QA% codex --no-daemon\n",
        "",
        "",
        terminal.managed_agent_launch_pending(),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), detection.state);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(4), false);
    assert!(
        terminal.managed_agent_launch_pending(),
        "a promptless process must not lose its startup watchdog"
    );
    assert!(!terminal.managed_agent_interactive_ready());
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(31), false);
    assert!(!terminal.managed_agent_interactive_ready());
    assert!(!terminal.managed_agent_launch_pending());
}

#[test]
fn accepted_session_start_confirms_startup_without_a_ready_screen_rule() {
    use crate::agents::resume::catalog::AgentSessionRef;

    for (kind, label, source) in [
        (AgentKind::Codex, "codex", "startup"),
        (AgentKind::Claude, "claude", "startup"),
        (AgentKind::Cursor, "cursor", "new"),
        (AgentKind::Grok, "grok", "new"),
        (AgentKind::Grok, "grok", "load"),
    ] {
        for state in [
            AgentState::Unknown,
            AgentState::Working,
            AgentState::Blocked,
        ] {
            let mut terminal = test_terminal();
            let now = Instant::now();
            terminal.begin_managed_agent(
                "fresh".into(),
                kind,
                now,
                Duration::from_secs(3),
                Duration::from_secs(30),
            );
            terminal.set_detected_state(Some(kind), state);
            terminal
                .set_agent_session_ref_for_session_start(
                    format!("herdr:{label}"),
                    label.into(),
                    AgentSessionRef::id("fresh-session"),
                    Some(1),
                    Some(source.into()),
                    &[],
                )
                .unwrap();
            terminal.reconcile_managed_agent_at(now + Duration::from_secs(31), false);
            assert!(
                terminal.managed_agent_interactive_ready(),
                "{label}: {state:?}"
            );
            assert!(!terminal.managed_agent_launch_pending());
            assert_eq!(terminal.state, state, "readiness must preserve activity");
        }
    }
}

#[test]
fn cached_or_identity_only_session_does_not_confirm_managed_startup() {
    use crate::agents::resume::catalog::{AgentSessionRef, PersistedAgentSession};

    for (cached, source, start_source) in [
        (true, "herdr:codex", Some("startup")),
        (false, "herdr:codex", None),
        (false, "custom", Some("startup")),
    ] {
        let mut terminal = test_terminal();
        let now = Instant::now();
        terminal.begin_managed_agent(
            "unconfirmed".into(),
            AgentKind::Codex,
            now,
            Duration::from_secs(3),
            Duration::from_secs(30),
        );
        terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Unknown);
        if cached {
            terminal.set_persisted_agent_session(PersistedAgentSession {
                source: source.into(),
                agent: "codex".into(),
                session_ref: AgentSessionRef::id("cached-session").unwrap(),
            });
        } else {
            terminal
                .set_agent_session_ref_for_session_start(
                    source.into(),
                    "codex".into(),
                    AgentSessionRef::id("identity-only"),
                    Some(1),
                    start_source.map(str::to_string),
                    &[],
                )
                .unwrap();
        }
        terminal.reconcile_managed_agent_at(now + Duration::from_secs(4), false);
        assert!(
            terminal.managed_agent_launch_pending(),
            "{cached}, {source}"
        );
        assert!(!terminal.managed_agent_interactive_ready());
    }
}

#[test]
fn codex_current_hook_review_keeps_managed_identity_while_waiting() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let screen = "Hooks need review\n3 hooks are new or changed.\n\
        Hooks can run outside the sandbox after you trust them.\n\n\
        › 1. Review hooks\n  2. Trust all and continue\n\
          3. Continue without trusting (hooks won't run)\n\n\
          enter confirm · esc skip\n";
    let detection = crate::agents::detect_agent_with_launch_context(
        Some(AgentKind::Codex),
        screen,
        "",
        "",
        true,
    );
    assert_eq!(detection.state, AgentState::Blocked);
    assert!(detection.visible_blocker);
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.begin_managed_agent(
        "reviewing-hooks".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(3),
        Duration::from_secs(30),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), detection.state);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(4), false);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(31), false);
    assert_eq!(terminal.agent_name.as_deref(), Some("reviewing-hooks"));
    assert!(terminal.managed_agent_launch_pending());
    assert!(!terminal.managed_agent_interactive_ready());
}

#[test]
fn managed_launch_signal_tracks_blocked_active_timeout_and_exit() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let mut terminal = test_terminal();
    let signal = Arc::new(AtomicBool::new(false));
    terminal.bind_managed_launch_pending(signal.clone());
    let now = Instant::now();
    terminal.begin_managed_agent(
        "builder".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(1),
        Duration::from_secs(30),
    );
    assert!(signal.load(Ordering::Acquire));
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(2), false);
    assert!(
        signal.load(Ordering::Acquire),
        "a startup dialog keeps the launch window open"
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(3), false);
    assert!(
        !signal.load(Ordering::Acquire),
        "interactive readiness closes the window"
    );
    let screen = "› Ask Codex to do anything\n  GPT-6.1-Sol xhigh · /repo\n  ? for shortcuts\n";
    assert_eq!(
        crate::agents::detect_agent_with_launch_context(
            Some(AgentKind::Codex),
            screen,
            "⠋ Codex",
            "",
            signal.load(Ordering::Acquire)
        )
        .state,
        AgentState::Working
    );
    for process_exited in [false, true] {
        terminal.begin_managed_agent(
            "builder".into(),
            AgentKind::Codex,
            now,
            Duration::from_secs(1),
            Duration::from_secs(30),
        );
        assert!(signal.load(Ordering::Acquire));
        terminal.reconcile_managed_agent_at(now + Duration::from_secs(31), process_exited);
        assert!(
            !signal.load(Ordering::Acquire),
            "timeout/exit must close the window"
        );
    }
    terminal.begin_managed_agent(
        "builder".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(1),
        Duration::from_secs(30),
    );
    terminal.clear_agent_name();
    assert!(!signal.load(Ordering::Acquire));
    terminal.restore_managed_agent("resumed".into(), AgentKind::Codex);
    assert!(
        !signal.load(Ordering::Acquire),
        "an already-active restore is not a pending launch"
    );
}

#[test]
fn bus_adopted_session_report_does_not_confirm_managed_startup() {
    use crate::agents::resume::catalog::AgentSessionRef;

    for (kind, label) in [
        (AgentKind::Codex, "codex"),
        (AgentKind::Claude, "claude"),
        (AgentKind::Cursor, "cursor"),
    ] {
        let mut terminal = test_terminal();
        let now = Instant::now();
        terminal.begin_managed_agent(
            "adopted".into(),
            kind,
            now,
            Duration::from_secs(3),
            Duration::from_secs(30),
        );
        // What the coordinator's report_adopted_session sends right after
        // AgentStarted on recovery or resume: no seq, no reporter process.
        terminal
            .set_agent_session_ref_for_session_start(
                format!("herdr:{label}"),
                label.into(),
                AgentSessionRef::id("saved-session"),
                None,
                Some("resume".into()),
                &[],
            )
            .unwrap();
        assert!(
            terminal.managed_agent_launch_pending(),
            "{label}: a Bus-synthesized cached-session report is not provider readiness"
        );
        assert!(!terminal.managed_agent_interactive_ready(), "{label}");
    }
}

#[test]
fn resumed_cursor_confirms_readiness_only_after_its_composer_appears() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.begin_managed_agent(
        "resumed-cursor".into(),
        AgentKind::Cursor,
        now,
        Duration::from_secs(3),
        Duration::from_secs(30),
    );
    terminal
        .set_agent_session_ref_for_session_start(
            "herdr:cursor".into(),
            "cursor".into(),
            crate::agents::resume::catalog::AgentSessionRef::id("saved-session"),
            None,
            Some("resume".into()),
            &[],
        )
        .unwrap();
    assert!(terminal.managed_agent_launch_pending());
    let screen = " → Add a follow-up\n\n  Grok 4.7 256K High  Run Everything\n  /workspace\n";
    let detection = crate::agents::detect_agent_with_launch_context(
        Some(AgentKind::Cursor),
        screen,
        "",
        "",
        terminal.managed_agent_launch_pending(),
    );
    assert!(detection.visible_idle);
    terminal.set_detected_state(Some(AgentKind::Cursor), detection.state);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(4), false);
    assert!(terminal.managed_agent_interactive_ready());
    assert!(!terminal.managed_agent_launch_pending());
    assert_eq!(
        terminal
            .persisted_agent_session
            .as_ref()
            .unwrap()
            .session_ref
            .value,
        "saved-session"
    );
}
#[test]
fn grok_saved_identity_is_not_ready_before_the_resume_process_starts() {
    for kind in [AgentKind::Grok, AgentKind::Claude] {
        let mut terminal = test_terminal();
        terminal.restore_managed_agent("restored".into(), kind);
        terminal.pending_agent_resume_plan =
            Some(crate::agents::resume::catalog::AgentResumePlan {
                agent: crate::agents::agent_label(kind).into(),
                argv: vec!["not-started".into()],
                dedupe_key: "saved-session".into(),
            });
        assert!(terminal.managed_agent_launch_pending());
        assert!(!terminal.managed_agent_interactive_ready());
        terminal.pending_agent_resume_plan = None;
        assert!(terminal.managed_agent_interactive_ready());
    }
}
