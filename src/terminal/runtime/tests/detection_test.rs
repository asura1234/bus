use super::*;

#[test]
fn foreground_shell_reports_process_exit_before_clearing_agent() {
    assert_eq!(
        foreground_shell_agent_action(Some(AgentKind::Codex), None, true, false),
        ForegroundShellAgentAction::ReportProcessExit
    );
    assert_eq!(
        foreground_shell_agent_action(Some(AgentKind::Codex), None, true, true),
        ForegroundShellAgentAction::ClearAgent
    );
}

#[test]
fn same_agent_after_reported_exit_is_a_replacement_process() {
    assert_eq!(
        foreground_shell_agent_action(Some(AgentKind::Pi), Some(AgentKind::Pi), false, true),
        ForegroundShellAgentAction::ReportReplacementProcess
    );
}

#[test]
fn unknown_non_shell_foreground_job_is_not_immediate_clear_signal() {
    assert_eq!(
        foreground_shell_agent_action(Some(AgentKind::Claude), None, false, false),
        ForegroundShellAgentAction::ObserveProbe
    );
}

#[tokio::test]
async fn first_agent_acquisition_keeps_osc_evidence_replacement_clears_it() {
    let runtime = TerminalRuntime::test_with_screen_bytes(80, 24, b"");
    runtime.test_process_pty_bytes(b"\x1b]2;startup title\x1b\\\x1b]9;4;1;\x1b\\");

    clear_osc_evidence_for_agent_transition(&runtime.terminal, None);
    assert_eq!(runtime.terminal.agent_osc_title(), "startup title");
    assert_eq!(runtime.terminal.agent_osc_progress(), "4;1;");

    clear_osc_evidence_for_agent_transition(&runtime.terminal, Some(AgentKind::Claude));
    assert_eq!(runtime.terminal.agent_osc_title(), "");
    assert_eq!(runtime.terminal.agent_osc_progress(), "");
}

#[test]
fn reported_process_exit_clears_before_unknown_foreground_probe() {
    assert_eq!(
        foreground_shell_agent_action(Some(AgentKind::Claude), None, false, true),
        ForegroundShellAgentAction::ClearAgent
    );
}

#[test]
fn foreground_agent_job_is_not_clear_signal() {
    assert_eq!(
        foreground_shell_agent_action(
            Some(AgentKind::Claude),
            Some(AgentKind::OpenCode),
            true,
            false,
        ),
        ForegroundShellAgentAction::ObserveProbe
    );
}

#[test]
fn foreground_agent_hint_accepts_pane_shell_environment() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 42,
        processes: vec![foreground_process(42, "bash")],
    };

    assert_eq!(
        agent_hint_for_foreground_job_members(&job, |pid| {
            (pid == 42).then_some(AgentKind::Claude)
        }),
        Some(AgentKind::Claude)
    );
}

#[test]
fn foreground_agent_hint_accepts_non_leader_foreground_process_environment() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 99,
        processes: vec![
            foreground_process(99, "fence"),
            foreground_process(100, "pi"),
        ],
    };

    assert_eq!(
        agent_hint_for_foreground_job_members(&job, |pid| {
            (pid == 100).then_some(AgentKind::Codex)
        }),
        Some(AgentKind::Codex)
    );
}

#[test]
fn foreground_agent_hint_wins_over_process_name_detection() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 99,
        processes: vec![foreground_process(99, "codex")],
    };

    let result = probe_foreground_process_from_jobs(
        42,
        Some(99),
        Some(job),
        || None,
        |pid| (pid == 99).then_some(AgentKind::Claude),
    );

    assert_eq!(result.agent, Some(AgentKind::Claude));
    assert_eq!(result.process_name.as_deref(), Some("claude"));
}

#[test]
fn foreground_agent_hint_on_inherited_child_environment_is_authoritative() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 99,
        processes: vec![foreground_process(99, "vim")],
    };

    let result = probe_foreground_process_from_jobs(
        42,
        Some(99),
        None,
        || Some(job),
        |pid| (pid == 99).then_some(AgentKind::Claude),
    );

    assert_eq!(result.agent, Some(AgentKind::Claude));
    assert_eq!(result.process_name.as_deref(), Some("claude"));
}

#[test]
fn non_leader_agent_hint_does_not_override_identifiable_leader() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 99,
        processes: vec![
            foreground_process(99, "codex"),
            foreground_process(100, "vim"),
        ],
    };

    let result = probe_foreground_process_from_jobs(
        42,
        Some(99),
        None,
        || Some(job),
        |pid| (pid == 100).then_some(AgentKind::Claude),
    );

    assert_eq!(result.agent, Some(AgentKind::Codex));
    assert_eq!(result.process_name.as_deref(), Some("codex"));
}

#[test]
fn non_leader_agent_hint_wins_when_leader_is_unidentified() {
    let job = crate::platform::ForegroundJob {
        process_group_id: 99,
        processes: vec![
            foreground_process(99, "some_vm"),
            foreground_process(100, "vim"),
        ],
    };

    let result = probe_foreground_process_from_jobs(
        42,
        Some(99),
        None,
        || Some(job),
        |pid| (pid == 100).then_some(AgentKind::Claude),
    );

    assert_eq!(result.agent, Some(AgentKind::Claude));
    assert_eq!(result.process_name.as_deref(), Some("claude"));
}

#[test]
fn windows_foreground_observation_schedule_preserves_safety_checks() {
    let before_safety_bound = PROCESS_RECHECK_IDENTIFIED - std::time::Duration::from_millis(1);
    let quiet = ProcessProbeInput {
        current_agent: Some(AgentKind::Codex),
        elapsed_since_process_check: before_safety_bound,
        ..process_probe_input()
    };
    let content_retry = std::time::Duration::from_millis(300);
    let content_due = |last: Option<u64>, current, elapsed| {
        last != Some(current) && (last.is_some() || elapsed >= content_retry)
    };

    assert!(!should_observe_foreground_process_group(false, quiet));
    assert!(should_observe_foreground_process_group(true, quiet));
    assert!(content_due(Some(0), 1, std::time::Duration::ZERO));
    assert!(!content_due(
        None,
        1,
        content_retry - std::time::Duration::from_millis(1)
    ));
    assert!(content_due(None, 1, content_retry));
    assert!(should_observe_foreground_process_group(
        false,
        ProcessProbeInput {
            elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED,
            ..quiet
        }
    ));

    for immediate in [
        ProcessProbeInput {
            has_process_probe: false,
            ..quiet
        },
        ProcessProbeInput {
            current_agent: None,
            acquisition_age: Some(std::time::Duration::ZERO),
            ..quiet
        },
        ProcessProbeInput {
            pending_restore_probe: true,
            ..quiet
        },
        ProcessProbeInput {
            pending_foreground_shell_clear: true,
            ..quiet
        },
    ] {
        assert!(should_observe_foreground_process_group(false, immediate));
    }
}

#[test]
fn unchanged_unidentified_foreground_group_skips_full_process_probe() {
    assert!(!should_probe_foreground_job(process_probe_input()));
}

#[test]
fn unidentified_foreground_group_change_runs_full_process_probe() {
    assert!(should_probe_foreground_job(ProcessProbeInput {
        foreground_pgid: Some(43),
        ..process_probe_input()
    }));
}

#[test]
fn unidentified_pane_gets_initial_process_probe() {
    assert!(should_probe_foreground_job(ProcessProbeInput {
        has_process_probe: false,
        ..process_probe_input()
    }));
}

#[test]
fn stable_unidentified_foreground_group_has_no_safety_process_probe() {
    assert!(!should_probe_foreground_job(ProcessProbeInput {
        elapsed_since_process_check: PROCESS_RECHECK_MISSING_FOREGROUND_GROUP,
        ..process_probe_input()
    }));
}

#[test]
fn unidentified_pane_without_foreground_group_uses_safety_process_probe() {
    assert!(!should_probe_foreground_job(ProcessProbeInput {
        foreground_pgid: None,
        last_foreground_pgid: None,
        ..process_probe_input()
    }));
    assert!(should_probe_foreground_job(ProcessProbeInput {
        foreground_pgid: None,
        last_foreground_pgid: None,
        elapsed_since_process_check: PROCESS_RECHECK_MISSING_FOREGROUND_GROUP,
        ..process_probe_input()
    }));
}

#[test]
fn unidentified_pane_probes_when_foreground_group_disappears() {
    assert!(should_probe_foreground_job(ProcessProbeInput {
        foreground_pgid: None,
        last_foreground_pgid: Some(42),
        ..process_probe_input()
    }));
}

#[test]
fn inferred_group_does_not_trigger_a_probe_on_every_tick() {
    let tracked = process_group_for_change_tracking(None, Some(300));
    assert_eq!(tracked, None);
    assert!(!should_probe_foreground_job(ProcessProbeInput {
        current_agent: Some(AgentKind::Claude),
        foreground_pgid: None,
        last_foreground_pgid: tracked,
        elapsed_since_process_check: std::time::Duration::from_millis(300),
        ..process_probe_input()
    }));
}

#[test]
fn pending_shell_clear_and_restore_force_process_probes() {
    assert!(should_probe_foreground_job(ProcessProbeInput {
        current_agent: Some(AgentKind::Codex),
        pending_foreground_shell_clear: true,
        ..process_probe_input()
    }));
    assert!(should_probe_foreground_job(ProcessProbeInput {
        current_agent: Some(AgentKind::Codex),
        pending_restore_probe: true,
        ..process_probe_input()
    }));
}

#[test]
fn acquisition_window_catches_delayed_same_group_wrapper_startup() {
    assert!(!should_probe_foreground_job(ProcessProbeInput {
        current_agent: None,
        acquisition_age: Some(std::time::Duration::from_millis(1250)),
        elapsed_since_process_check: PROCESS_ACQUISITION_FAST_RECHECK
            - std::time::Duration::from_millis(1),
        ..process_probe_input()
    }));
    assert!(should_probe_foreground_job(ProcessProbeInput {
        current_agent: None,
        acquisition_age: Some(std::time::Duration::from_millis(1250)),
        elapsed_since_process_check: PROCESS_ACQUISITION_FAST_RECHECK,
        ..process_probe_input()
    }));
    assert!(should_probe_foreground_job(ProcessProbeInput {
        current_agent: None,
        acquisition_age: Some(std::time::Duration::from_secs(5)),
        elapsed_since_process_check: PROCESS_ACQUISITION_SLOW_RECHECK,
        ..process_probe_input()
    }));
    assert!(!should_probe_foreground_job(ProcessProbeInput {
        current_agent: None,
        acquisition_age: Some(PROCESS_ACQUISITION_WINDOW + std::time::Duration::from_millis(1),),
        elapsed_since_process_check: PROCESS_ACQUISITION_SLOW_RECHECK,
        ..process_probe_input()
    }));
}

#[test]
fn content_change_starts_bounded_unidentified_acquisition_window() {
    let now = std::time::Instant::now();
    let mut acquisition_started_at = None;
    let mut last_content_change_at = None;

    sync_content_change_acquisition(
        None,
        false,
        true,
        now,
        &mut acquisition_started_at,
        &mut last_content_change_at,
    );
    assert_eq!(acquisition_started_at, Some(now));
    assert_eq!(last_content_change_at, Some(now));

    let later = now + std::time::Duration::from_secs(1);
    sync_content_change_acquisition(
        None,
        false,
        true,
        later,
        &mut acquisition_started_at,
        &mut last_content_change_at,
    );
    assert_eq!(
        acquisition_started_at,
        Some(now),
        "changed frames should not refresh the acquisition window"
    );
    assert_eq!(last_content_change_at, Some(later));

    let quiet_after_window = later + PROCESS_ACQUISITION_WINDOW + PROCESS_ACQUISITION_IDLE_RESET;
    sync_content_change_acquisition(
        None,
        false,
        false,
        quiet_after_window,
        &mut acquisition_started_at,
        &mut last_content_change_at,
    );
    assert_eq!(acquisition_started_at, None);
    assert_eq!(last_content_change_at, None);

    let next_burst = quiet_after_window + std::time::Duration::from_secs(1);
    sync_content_change_acquisition(
        None,
        false,
        true,
        next_burst,
        &mut acquisition_started_at,
        &mut last_content_change_at,
    );
    assert_eq!(acquisition_started_at, Some(next_burst));
    assert_eq!(last_content_change_at, Some(next_burst));
}

#[test]
fn content_change_does_not_start_acquisition_when_process_probe_has_other_signal() {
    let now = std::time::Instant::now();
    let mut acquisition_started_at = None;
    let mut last_content_change_at = None;

    sync_content_change_acquisition(
        Some(AgentKind::Codex),
        false,
        true,
        now,
        &mut acquisition_started_at,
        &mut last_content_change_at,
    );
    assert_eq!(acquisition_started_at, None);
    assert_eq!(last_content_change_at, None);

    sync_content_change_acquisition(
        None,
        true,
        true,
        now,
        &mut acquisition_started_at,
        &mut last_content_change_at,
    );
    assert_eq!(acquisition_started_at, None);
    assert_eq!(last_content_change_at, None);
}

#[test]
fn content_change_restarts_stale_process_group_acquisition_window() {
    let now = std::time::Instant::now();
    let stale_start = now - PROCESS_ACQUISITION_WINDOW - std::time::Duration::from_millis(1);
    let mut acquisition_started_at = Some(stale_start);
    let mut last_content_change_at = None;

    sync_content_change_acquisition(
        None,
        false,
        true,
        now,
        &mut acquisition_started_at,
        &mut last_content_change_at,
    );

    assert_eq!(acquisition_started_at, Some(now));
    assert_eq!(last_content_change_at, Some(now));
}

#[test]
fn identified_agent_uses_shorter_safety_process_probe() {
    assert!(!should_probe_foreground_job(ProcessProbeInput {
        current_agent: Some(AgentKind::Codex),
        elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED
            - std::time::Duration::from_millis(1),
        ..process_probe_input()
    }));
    assert!(should_probe_foreground_job(ProcessProbeInput {
        current_agent: Some(AgentKind::Codex),
        elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED,
        ..process_probe_input()
    }));
}

#[test]
fn identified_agent_probes_when_foreground_group_disappears() {
    assert!(should_probe_foreground_job(ProcessProbeInput {
        current_agent: Some(AgentKind::Codex),
        foreground_pgid: None,
        last_foreground_pgid: Some(42),
        elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED
            - std::time::Duration::from_millis(1),
        ..process_probe_input()
    }));
}

#[test]
fn stable_missing_foreground_group_uses_safety_process_probe() {
    assert!(!should_probe_foreground_job(ProcessProbeInput {
        current_agent: Some(AgentKind::Codex),
        foreground_pgid: None,
        last_foreground_pgid: None,
        elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED
            - std::time::Duration::from_millis(1),
        ..process_probe_input()
    }));
    assert!(should_probe_foreground_job(ProcessProbeInput {
        current_agent: Some(AgentKind::Codex),
        foreground_pgid: None,
        last_foreground_pgid: None,
        elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED,
        ..process_probe_input()
    }));
}

#[test]
fn transient_process_miss_keeps_current_agent_detected() {
    let mut presence = AgentDetectionPresence::from_agent(Some(AgentKind::Pi));

    let changed = presence.observe_process_probe(None);

    assert!(!changed, "one miss should not clear the detected agent");
    assert_eq!(presence.current_agent(), Some(AgentKind::Pi));
}

#[test]
fn agent_only_clears_after_confirmation_misses() {
    let mut presence = AgentDetectionPresence::from_agent(Some(AgentKind::Pi));

    for attempt in 1..AGENT_MISS_CONFIRMATION_ATTEMPTS {
        let changed = presence.observe_process_probe(None);
        assert!(
            !changed,
            "miss {attempt} should stay in the confirmation window"
        );
        assert_eq!(presence.current_agent(), Some(AgentKind::Pi));
    }

    let changed = presence.observe_process_probe(None);
    assert!(changed, "last confirmation miss should clear the agent");
    assert_eq!(presence.current_agent(), None);
}
