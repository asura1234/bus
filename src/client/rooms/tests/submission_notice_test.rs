use super::*;

const DETAIL: &str = "The draft was left because it was edited. Inspect the transcript and composer before resending.";

fn submitting() -> (BusUi, RoomId, AgentId, RequestId) {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    snapshot.state.confirm_hook_setup(agent).unwrap();
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Idle, 1000)
        .unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.set_draft_text(room, "brief").unwrap();
    let request = snapshot.state.submit_draft(room, 1000).unwrap()[0];
    snapshot
        .state
        .begin_submission(request, "launch", 0)
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    (ui, room, agent, request)
}

fn failed(ui: &mut BusUi, request: RequestId) {
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .record_submission(
            request,
            SubmissionOutcome::Uncertain {
                message: DETAIL.into(),
            },
        )
        .unwrap();
    snapshot
        .state
        .fail_unconfirmed_submissions(1000 + DELIVERED_STALL_MS)
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

#[test]
fn provider_flake_started_receipt_does_not_report_failed_submission_to_client() {
    use crate::messaging::model::{CallbackDisposition, CallbackEventKind, ProviderCallback};
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let agent = snapshot
        .state
        .create_agent(room, "kimi", Provider::Kimi, "/repo".into(), None)
        .unwrap();
    snapshot
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    snapshot.state.confirm_hook_setup(agent).unwrap();
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Idle, 1000)
        .unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.set_draft_text(room, "native task").unwrap();
    let request = snapshot.state.submit_draft(room, 1000).unwrap()[0];
    snapshot
        .state
        .begin_submission(request, "launch", 10)
        .unwrap();
    snapshot
        .state
        .record_submission(
            request,
            SubmissionOutcome::Uncertain {
                message: "UNCONFIRMED_BEFORE_RECEIPT".into(),
            },
        )
        .unwrap();
    let payload = snapshot.state.render_request_payload(request).unwrap();
    assert_eq!(
        snapshot.state.accept_callback(ProviderCallback {
            callback_id: "started-receipt".into(),
            sequence: 11,
            occurred_at_ms: 1001,
            agent_id: agent,
            launch_id: "launch".into(),
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("1".into()),
            provider_prompt_id: None,
            prompt_payload: Some(payload),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    assert!(snapshot.state.request(request).unwrap().trusted_start_bound);
    // A client may first observe the request only after its native receipt.
    snapshot.state = serde_json::from_slice(&serde_json::to_vec(&snapshot.state).unwrap()).unwrap();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
    });
    assert!(
        !capture.text().contains("bus.request.failed"),
        "{}",
        capture.text()
    );
}

#[test]
fn abandoned_submission_history_never_draws_recovery_after_later_agent_errors() {
    let (mut ui, room, agent, request) = submitting();
    let mut history = super::super::history::History::default();
    let mut thumbnails = super::super::thumbnails::Thumbnails::default();
    history.lines(
        &ui.snapshot.state,
        ui.snapshot.state.room(room).unwrap(),
        200,
        1,
        2000,
        &mut thumbnails,
    );
    failed(&mut ui, request);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_agent_error(agent, Some("a later unrelated error".into()))
        .unwrap();
    // The request outcome must survive snapshot persistence, not borrow the agent's latest error.
    snapshot.state = serde_json::from_slice(&serde_json::to_vec(&snapshot.state).unwrap()).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let rendered = history
        .lines(
            &ui.snapshot.state,
            ui.snapshot.state.room(room).unwrap(),
            200,
            3,
            2000,
            &mut thumbnails,
        )
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for text in [
        "submission_unconfirmed",
        "draft was left",
        "before resending",
    ] {
        assert!(!rendered.contains(text), "rendered {text}: {rendered}");
    }
    assert!(
        !rendered.contains("a later unrelated error"),
        "outcomes belong to their request"
    );
}

#[test]
fn an_uncertain_submission_never_titles_the_composer() {
    let (mut ui, _, _, request) = submitting();
    failed(&mut ui, request);
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let mut title = String::new();
    for y in composer_rect(&ui).y..30 {
        for x in 0..100 {
            title.push_str(buffer[(x, y)].symbol());
        }
    }
    assert!(!title.contains("draft was left"), "{title}");
    assert!(!title.contains("submission_unconfirmed"), "{title}");
}

#[test]
fn expanded_details_keep_uncertain_submission_recovery_in_the_log_only() {
    let (mut ui, _, agent, request) = submitting();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        failed(&mut ui, request);
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .set_agent_details_disclosed(agent, true)
            .unwrap();
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
    });
    ui.compute_view(160, 60);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 160, 60));
    ui.render(&mut buffer);
    let mut text = String::new();
    for y in ui.view.sidebar_body.y..ui.view.sidebar_body.bottom() {
        for x in 1..ui.view.sidebar.right() - 1 {
            text.push_str(buffer[(x, y)].symbol());
        }
    }
    for expected in ["draftwasleft", "beforeresending"] {
        assert!(!text.replace(' ', "").contains(expected), "{text}");
    }
    let logs = capture.text();
    assert_eq!(logs.matches("bus.agent.notice").count(), 1, "{logs}");
    assert!(logs.contains(DETAIL), "{logs}");
}

fn tui_error_probe_screen(ui: &mut BusUi) -> String {
    ui.compute_view(180, 60);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 180, 60));
    ui.render(&mut buffer);
    (0..60)
        .map(|y| {
            (0..180)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tui_error_text_binding_rejects_history_submission_errors() {
    let (mut ui, _, _, request) = submitting();
    failed(&mut ui, request);
    let screen = tui_error_probe_screen(&mut ui);
    assert!(
        !screen.contains("The draft was left"),
        "binding rule forbids error/recovery text on every TUI surface; rendered frame:\n{screen}"
    );
}

#[test]
fn tui_error_text_definite_rejection_logs_not_renders() {
    const CAUSE: &str = "Agent is not idle; prompt was not sent";
    let (mut ui, _, _, request) = submitting();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .record_submission(
                request,
                SubmissionOutcome::DefinitelyRejected {
                    message: CAUSE.into(),
                },
            )
            .unwrap();
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
    });
    let screen = tui_error_probe_screen(&mut ui);
    assert!(
        !screen.contains(CAUSE),
        "definite native refusal must not render: {screen}"
    );
    assert!(
        capture.text().contains(CAUSE),
        "native refusal must reach client log: {}",
        capture.text()
    );
}

fn orchestrate(state: &mut BusState, room: RoomId) -> AgentId {
    let master = state.ensure_master_room();
    let orchestrator = state
        .create_agent(master, "lead", Provider::Codex, "/project".into(), None)
        .unwrap();
    state.bind_orchestrator(orchestrator, room).unwrap();
    orchestrator
}

fn assert_no_diagnostics(ui: &mut BusUi, forbidden: &[&str]) {
    for (cols, rows) in [(80, 24), (180, 60)] {
        for expanded in [false, true] {
            let mut snapshot = (*ui.snapshot).clone();
            for agent in snapshot.state.agents().map(|a| a.id).collect::<Vec<_>>() {
                snapshot
                    .state
                    .set_agent_details_disclosed(agent, expanded)
                    .unwrap();
            }
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
            let frame = room_screen(ui, cols, rows);
            for text in forbidden {
                assert!(
                    !frame.contains(text),
                    "rendered {text} at {cols}x{rows}, expanded={expanded}:\n{frame}"
                );
            }
        }
    }
}

#[test]
fn tui_error_text_startup_failure_and_its_routed_notice_are_log_only() {
    use crate::messaging::model::{AgentStartup, StartupFailure};
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    orchestrate(&mut snapshot.state, room);
    snapshot.state.set_draft_text(room, "launch task").unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    let request = snapshot.state.submit_draft(room, 1000).unwrap()[0];
    snapshot
        .state
        .set_agent_startup(
            agent,
            Some(AgentStartup {
                command: "codex".into(),
                started_at_ms: 1000,
                reported: true,
                process_observed: false,
                native_terminal_id: None,
                recovery: None,
                failure: Some(StartupFailure {
                    reason: "STARTUP_CAUSE_SENTINEL".into(),
                    recovery: "RECOVERY_SENTINEL".into(),
                }),
            }),
        )
        .unwrap();
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Unavailable, 2000)
        .unwrap();
    snapshot.state.abandon_failed_startup_queue(agent, 2000);
    snapshot
        .state
        .route_worker_replies(5000, &Default::default())
        .unwrap();
    assert!(
        snapshot
            .state
            .requests()
            .any(|r| r.bus_generated && r.prompt.text.contains("STARTUP_CAUSE_SENTINEL")),
        "exercise the actual routed failure envelope"
    );
    snapshot.revision += 1;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| ui.receive_snapshot(Arc::new(snapshot)));
    assert_no_diagnostics(
        &mut ui,
        &[
            "STARTUP_CAUSE_SENTINEL",
            "RECOVERY_SENTINEL",
            "agent_startup_failed",
            "No reply",
        ],
    );
    let logs = capture.text();
    assert!(logs.contains("STARTUP_CAUSE_SENTINEL"), "{logs}");
    assert!(
        logs.contains(&format!("request_id={}", request.0)),
        "{logs}"
    );
    assert!(room_screen(&mut ui, 180, 60).contains("Unavailable"));
}

#[test]
fn tui_error_text_provider_failure_envelopes_stay_out_of_history_after_archive() {
    use crate::messaging::model::{RecordedTurnEnd, TURN_SETTLE_MS};
    let (mut ui, room, agent, request) = submitting();
    let mut snapshot = (*ui.snapshot).clone();
    orchestrate(&mut snapshot.state, room);
    snapshot
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Working, 1100)
        .unwrap();
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Idle, 1200)
        .unwrap();
    assert_eq!(
        snapshot
            .state
            .settle_ended_request(agent, 1200 + TURN_SETTLE_MS, |_| Some(
                RecordedTurnEnd::Error("PROVIDER_CAUSE_SENTINEL".into())
            )),
        Some(request)
    );
    snapshot
        .state
        .route_worker_replies(1200 + TURN_SETTLE_MS + 3000, &Default::default())
        .unwrap();
    assert!(
        snapshot
            .state
            .requests()
            .any(|r| r.bus_generated && r.prompt.text.contains("PROVIDER_CAUSE_SENTINEL")),
        "exercise the actual routed provider failure"
    );
    snapshot
        .state
        .set_agent_error(agent, Some("LATER_CAUSE_SENTINEL".into()))
        .unwrap();
    snapshot.state.archive(u64::MAX / 2);
    snapshot.revision += 1;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| ui.receive_snapshot(Arc::new(snapshot)));
    assert_no_diagnostics(
        &mut ui,
        &[
            "PROVIDER_CAUSE_SENTINEL",
            "LATER_CAUSE_SENTINEL",
            "provider_error",
            "No reply",
        ],
    );
    let logs = capture.text();
    for text in ["PROVIDER_CAUSE_SENTINEL", "LATER_CAUSE_SENTINEL"] {
        assert!(
            logs.contains(text),
            "durable and current errors both reach the client log: {logs}"
        );
    }
    assert!(
        logs.contains(&format!("request_id={}", request.0)),
        "{logs}"
    );
}

#[test]
fn tui_error_text_exit_snapshot_failure_is_logged_without_an_exit_chat_body() {
    use crate::messaging::model::ExitSnapshot;
    let (mut ui, room, agent, _) = submitting();
    let mut snapshot = (*ui.snapshot).clone();
    let orchestrator = orchestrate(&mut snapshot.state, room);
    snapshot
        .state
        .submit_message_from(
            room,
            Draft {
                text: "exit task".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Agent(orchestrator),
            1100,
        )
        .unwrap();
    snapshot
        .state
        .record_exit_snapshot(
            agent,
            ExitSnapshot {
                launch_id: Some("launch".into()),
                terminal_id: "terminal".into(),
                path: None,
                error: Some("EXIT_CAUSE_SENTINEL".into()),
                tail: "EXIT_TAIL_SENTINEL".into(),
                at_ms: 2000,
                terminal_closed: None,
            },
        )
        .unwrap();
    snapshot.state.record_worker_exit(agent, 2000);
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Unavailable, 2000)
        .unwrap();
    snapshot
        .state
        .route_worker_replies(5000, &Default::default())
        .unwrap();
    assert!(
        snapshot
            .state
            .requests()
            .any(|r| r.bus_generated && r.prompt.text.contains("EXIT_CAUSE_SENTINEL")),
        "exercise the actual exit failure envelope"
    );
    snapshot.revision += 1;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| ui.receive_snapshot(Arc::new(snapshot)));
    assert_no_diagnostics(
        &mut ui,
        &[
            "EXIT_CAUSE_SENTINEL",
            "Exit snapshot unavailable",
            "Exited with open request",
        ],
    );
    assert!(
        capture.text().contains("EXIT_CAUSE_SENTINEL"),
        "{}",
        capture.text()
    );
    assert!(
        capture.text().contains("terminal_id=terminal"),
        "{}",
        capture.text()
    );
    assert!(room_screen(&mut ui, 180, 60).contains("Unavailable"));
}

#[test]
fn tui_error_text_missing_provider_has_only_unavailable_as_its_indication() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.confirm_hook_setup(agent).unwrap();
    snapshot
        .state
        .block_missing_binary(agent, "codex", 1000)
        .unwrap();
    snapshot.revision += 1;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| ui.receive_snapshot(Arc::new(snapshot)));
    assert_no_diagnostics(
        &mut ui,
        &[
            "BLOCKED:",
            "missing from PATH",
            "Restore",
            "bus agent resume",
        ],
    );
    let frame = room_screen(&mut ui, 180, 60);
    assert!(frame.contains("Unavailable"), "{frame}");
    assert!(
        capture.text().contains("missing from PATH"),
        "{}",
        capture.text()
    );
    assert!(
        capture.text().contains(&format!("room_id={}", room.0)),
        "{}",
        capture.text()
    );
}

#[test]
fn tui_error_text_save_failure_and_timeout_never_draw_a_footer() {
    for failed_command in [false, true] {
        let (mut ui, room, _) = fixture();
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("retained draft");
        ui.text_changed(room);
        ui.request_quit();
        if failed_command {
            let id = ui.pending.front().unwrap().id;
            ui.receive_event(BusEvent::CommandFinished {
                command_id: id,
                result: Err("SAVE_CAUSE_SENTINEL".into()),
            });
            let mut snapshot = (*ui.snapshot).clone();
            snapshot.last_command_id = id;
            ui.receive_snapshot(Arc::new(snapshot));
        }
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            ui.quitting = Some(std::time::Instant::now() - std::time::Duration::from_secs(6));
            ui.tick_quit(std::time::Instant::now());
        });
        assert!(
            ui.force_exit_until.is_some(),
            "keep the existing force-quit behavior"
        );
        assert_no_diagnostics(
            &mut ui,
            &[
                "SAVE_CAUSE_SENTINEL",
                "Ctrl+Q again",
                "May lose unsaved",
                "Unsaved changes",
                "slow storage",
                "retries saving",
            ],
        );
        ui.force_exit_until = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        ui.tick_quit(std::time::Instant::now());
        assert_no_diagnostics(
            &mut ui,
            &[
                "Ctrl+Q again",
                "May lose unsaved",
                "Unsaved changes",
                "slow storage",
                "retries saving",
            ],
        );
        assert!(
            capture.text().contains("bus.quit.warning"),
            "{}",
            capture.text()
        );
        assert!(room_screen(&mut ui, 180, 60).contains("retained draft"));
    }
}

#[test]
fn tui_error_text_dialog_notice_delivery_failure_keeps_only_the_question_visible() {
    use crate::messaging::model::DialogNoticeTo;
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let orchestrator = orchestrate(&mut snapshot.state, room);
    let notice = snapshot
        .state
        .submit_bus_message_with(
            room,
            Draft {
                text: "Choose the next action".into(),
                files: vec![],
                recipient_ids: [orchestrator].into(),
            },
            Author::Bus,
            1000,
            true,
        )
        .unwrap()[0];
    snapshot
        .state
        .set_dialog_notice_to(agent, Some(DialogNoticeTo::Orchestrator(notice)))
        .unwrap();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        snapshot
            .state
            .redirect_dialog_notice(notice, "DIALOG_CAUSE_SENTINEL", 2000)
            .unwrap();
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
    });
    assert_no_diagnostics(
        &mut ui,
        &[
            "DIALOG_CAUSE_SENTINEL",
            "did not receive",
            "Delivery to the orchestrator",
            "dialog state could not be read",
        ],
    );
    assert!(room_screen(&mut ui, 180, 60).contains("Choose the next action"));
    assert!(
        capture.text().contains("DIALOG_CAUSE_SENTINEL"),
        "{}",
        capture.text()
    );
}

#[test]
fn tui_error_text_latest_only_cache_cannot_restore_diagnostic_chat() {
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_bus_notice(room, None, "LEGACY_DIAGNOSTIC_SENTINEL".into(), 1000)
        .unwrap();
    let cached = snapshot.state.notices(room).next().unwrap().clone();
    let mut saved = serde_json::to_value(&snapshot.state).unwrap();
    saved["rooms"][room.0.to_string()]["notices"] = serde_json::json!([]);
    saved["rooms"][room.0.to_string()]["latest_prompt"] = serde_json::to_value(cached).unwrap();
    snapshot.state = serde_json::from_value(saved).unwrap();
    assert!(snapshot.state.notices(room).next().is_none());
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    assert_no_diagnostics(&mut ui, &["LEGACY_DIAGNOSTIC_SENTINEL"]);
}

#[test]
fn tui_error_text_restart_outcomes_never_draw_a_diagnostic() {
    for outcome in ["rolled_back", "refused", "rollback_failed", "restarted"] {
        let (mut ui, room, _) = fixture();
        let mut snapshot = (*ui.snapshot).clone();
        orchestrate(&mut snapshot.state, room);
        let record = crate::utils::restart_record::RestartLog {
            at_ms: 1000,
            who: "user:test".into(),
            old_sha: "a".repeat(40),
            new_sha: "b".repeat(40),
            old_binary: "/old".into(),
            new_binary: "/new".into(),
            outcome: outcome.into(),
            reason: "RESTART_CAUSE_SENTINEL".into(),
        };
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            snapshot.state.announce_restart(&record, 1000).unwrap();
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
            assert_no_diagnostics(
                &mut ui,
                &[
                    "RESTART_CAUSE_SENTINEL",
                    "did not become healthy",
                    "was refused",
                    "restart failed",
                ],
            );
        });
        let logs = capture.text();
        assert!(
            logs.contains("RESTART_CAUSE_SENTINEL") && logs.contains(outcome),
            "{logs}"
        );
        assert!(
            logs.contains(&record.old_sha) && logs.contains(&record.new_sha),
            "{logs}"
        );
        if outcome == "restarted" {
            assert!(room_screen(&mut ui, 180, 60).contains("Restarted"));
        }
    }
}

#[test]
fn retained_report_diagnostics_stay_logged_without_reports_about_reports() {
    use tracing_subscriber::prelude::*;
    let root = crate::utils::test_temp::unique_temp_path("bus-retained-report-diagnostic");
    let mut runtime = crate::utils::error_pipe::Runtime::default();
    runtime.configure(&root, Some("lead"));
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let state = &mut snapshot.state;
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 1000)
        .unwrap();
    let report = state
        .submit_message_with(
            room,
            Draft {
                text: "Bus error: original.failure. Logs: /logs/client.log".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Bus,
            1000,
            true,
        )
        .unwrap()[0];
    state.begin_submission(report, "launch", 0).unwrap();
    state
        .record_submission(
            report,
            SubmissionOutcome::DefinitelyRejected {
                message: "REPORT_DELIVERY_CAUSE".into(),
            },
        )
        .unwrap();
    assert!(state.request(report).unwrap().is_error_report());
    assert!(state.request(report).unwrap().failure_reason.is_some());
    state
        .observe_status(agent, RuntimeStatus::Idle, 2000)
        .unwrap();
    let ordinary = state
        .submit_message_with(
            room,
            Draft {
                text: "ordinary question".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            2000,
            true,
        )
        .unwrap()[0];
    state.begin_submission(ordinary, "launch", 0).unwrap();
    state
        .record_submission(
            ordinary,
            SubmissionOutcome::Uncertain {
                message: "ORDINARY_DELIVERY_CAUSE".into(),
            },
        )
        .unwrap();
    state
        .fail_unconfirmed_submissions(2000 + DELIVERED_STALL_MS)
        .unwrap();
    // Durable replay must retain identity; this is no longer the submission scope.
    snapshot.state = serde_json::from_slice(&serde_json::to_vec(&snapshot.state).unwrap()).unwrap();
    let path = root.join("herdr-client.log");
    let file = std::fs::File::create(&path).unwrap();
    let subscriber = tracing_subscriber::registry()
        .with(crate::utils::error_pipe::CaptureLayer::new(path.clone()))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(move || file.try_clone().unwrap()),
        );
    tracing::subscriber::with_default(subscriber, || {
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot.clone()));
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
    });
    let logs = std::fs::read_to_string(path).unwrap();
    assert!(logs.contains("error_report_delivery_failed"), "{logs}");
    assert!(logs.contains("ORDINARY_DELIVERY_CAUSE"), "{logs}");
    assert_eq!(logs.matches("bus.request.failed").count(), 2, "{logs}");
    let records = crate::utils::error_pipe::drain(&root).unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.event == "bus.request.failed")
            .count(),
        1,
        "only the ordinary request may enter capture: {records:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tui_error_text_error_reports_and_their_replies_are_never_chat() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let report = snapshot
        .state
        .submit_message_with(
            room,
            Draft {
                text: "Bus error: REPORT_EVENT_SENTINEL. Logs: /logs/client.log".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Bus,
            1000,
            true,
        )
        .unwrap()[0];
    snapshot
        .state
        .finish_for_test(report, "REPORT_REPLY_SENTINEL", 2000);
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot.clone()));
    assert_no_diagnostics(&mut ui, &["REPORT_EVENT_SENTINEL", "REPORT_REPLY_SENTINEL"]);
    assert!(snapshot
        .state
        .archive(2000 + crate::messaging::model::ARCHIVE_AFTER_MS + 1));
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    assert_no_diagnostics(&mut ui, &["REPORT_EVENT_SENTINEL", "REPORT_REPLY_SENTINEL"]);
}

fn snapshot_with_terminal_reason(reason: &str) -> (BusUi, BusSnapshot, RequestId) {
    let (ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let request = snapshot
        .state
        .submit_message_with(
            room,
            Draft {
                text: "retained outcome".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            1000,
            true,
        )
        .unwrap()[0];
    snapshot.state.recover_idle_request(request, 2000).unwrap();
    // Exercise the saved terminal reason consumed by the UI, including retirement
    // records from earlier versions. The request identity survives the round trip.
    let mut saved = serde_json::to_value(&snapshot.state).unwrap();
    saved["requests"][request.0.to_string()]["failure_reason"] = reason.into();
    snapshot.state = serde_json::from_value(saved).unwrap();
    (ui, snapshot, request)
}

#[test]
fn deliberate_request_outcomes_log_at_info_without_error_pipe_reports() {
    use tracing_subscriber::prelude::*;
    for reason in [
        "interrupted",
        "agent_deleted",
        "room_deleted",
        "github_watch_retired",
        "room_digest_retired",
        "messaging_onboarding_retired",
        "operational_notice_retired",
        "background_wait_superseded",
    ] {
        let root = crate::utils::test_temp::unique_temp_path("bus-deliberate-request-outcome");
        let mut runtime = crate::utils::error_pipe::Runtime::default();
        runtime.configure(&root, Some("lead"));
        let (mut ui, mut snapshot, request) = snapshot_with_terminal_reason(reason);
        let path = root.join("client.log");
        let file = std::fs::File::create(&path).unwrap();
        let subscriber = tracing_subscriber::registry()
            .with(crate::utils::error_pipe::CaptureLayer::new(path.clone()))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .without_time()
                    .with_writer(move || file.try_clone().unwrap()),
            );
        tracing::subscriber::with_default(subscriber, || {
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
        });
        let log = std::fs::read_to_string(path).unwrap();
        let records = crate::utils::error_pipe::drain(&root).unwrap();
        assert!(
            records.is_empty(),
            "deliberate {reason} was reported as an error: {records:?}"
        );
        assert!(
            log.contains("INFO")
                && log.contains(reason)
                && log.contains(&format!("request_id={}", request.0)),
            "{log}"
        );
        assert!(!log.contains("bus.request.failed"), "{log}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn opening_retained_request_failures_seeds_observation_but_new_causes_still_warn() {
    use tracing_subscriber::prelude::*;
    let root = crate::utils::test_temp::unique_temp_path("bus-retained-request-reopen");
    let mut runtime = crate::utils::error_pipe::Runtime::default();
    runtime.configure(&root, Some("lead"));
    let (_, mut snapshot, request) = snapshot_with_terminal_reason("provider_error: OLD_CAUSE");
    let path = root.join("client.log");
    let file = std::fs::File::create(&path).unwrap();
    let subscriber = tracing_subscriber::registry()
        .with(crate::utils::error_pipe::CaptureLayer::new(path.clone()))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(move || file.try_clone().unwrap()),
        );
    tracing::subscriber::with_default(subscriber, || {
        let mut ui = BusUi::new(Arc::new(snapshot.clone()));
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot.clone()));
        assert!(
            crate::utils::error_pipe::drain(&root).unwrap().is_empty(),
            "opening and receiving the same retained failure must not report it again"
        );
        let mut saved = serde_json::to_value(&snapshot.state).unwrap();
        saved["requests"][request.0.to_string()]["failure_reason"] =
            "provider_error: NEW_CAUSE".into();
        snapshot.state = serde_json::from_value(saved).unwrap();
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot.clone()));
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
    });
    let records = crate::utils::error_pipe::drain(&root).unwrap();
    assert_eq!(
        records.len(),
        1,
        "new cause must be reported once: {records:?}"
    );
    assert_eq!(records[0].event, "bus.request.failed");
    let log = std::fs::read_to_string(path).unwrap();
    assert!(!log.contains("OLD_CAUSE"), "{log}");
    assert_eq!(log.matches("NEW_CAUSE").count(), 1, "{log}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tui_error_text_hidden_envelopes_and_cache_never_enter_composer_recall() {
    for source in ["routing", "report", "cache_only", "normal_notice"] {
        let (mut ui, room, agent) = fixture();
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .submit_message_with(
                room,
                Draft {
                    text: "AUTHORED_RECALL_CONTROL".into(),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1000,
                true,
            )
            .unwrap();
        match source {
            "routing" => {
                let sender = snapshot
                    .state
                    .create_agent(room, "sender", Provider::Codex, "/project".into(), None)
                    .unwrap();
                snapshot
                    .state
                    .submit_bus_message_with(
                        room,
                        Draft {
                            text: "RECALL_DIAGNOSTIC_SENTINEL".into(),
                            files: vec![],
                            recipient_ids: [agent].into(),
                        },
                        Author::Agent(sender),
                        1001,
                        false,
                    )
                    .unwrap();
            }
            "report" => {
                snapshot
                    .state
                    .submit_message_with(
                        room,
                        Draft {
                            text: "Bus error: RECALL_DIAGNOSTIC_SENTINEL".into(),
                            files: vec![],
                            recipient_ids: [agent].into(),
                        },
                        Author::Bus,
                        1001,
                        true,
                    )
                    .unwrap();
            }

            "normal_notice" => {
                snapshot
                    .state
                    .post_bus_notice(room, None, "NORMAL_RECALL_NOTICE".into(), 1001)
                    .unwrap();
            }
            _ => {
                let mut saved = serde_json::to_value(&snapshot.state).unwrap();
                let prompt = &mut saved["rooms"][room.0.to_string()]["latest_prompt"];
                prompt["id"] = 999999.into();
                prompt["author"] = "bus".into();
                prompt["text"] = "RECALL_DIAGNOSTIC_SENTINEL".into();
                prompt["submitted_at_ms"] = 1001.into();
                snapshot.state = serde_json::from_value(saved).unwrap();
            }
        }
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
        key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
        let expected = match source {
            "normal_notice" => "NORMAL_RECALL_NOTICE",
            _ => "AUTHORED_RECALL_CONTROL",
        };
        assert_eq!(ui.locals[&room].text.text, expected, "{source}");
        assert_no_diagnostics(&mut ui, &["RECALL_DIAGNOSTIC_SENTINEL"]);
    }
}

#[test]
fn tui_error_text_uncertain_steering_retains_native_detail_in_log_only() {
    const CAUSE: &str = "STEERING_NATIVE_FAILURE_DETAIL_SENTINEL";
    let (mut ui, room, agent, lead) = submitting();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .record_submission(
            lead,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
    let payload = snapshot.state.render_request_payload(lead).unwrap();
    assert_eq!(
        snapshot.state.accept_callback(ProviderCallback {
            callback_id: "steering-start".into(),
            sequence: 1,
            occurred_at_ms: 1001,
            agent_id: agent,
            launch_id: "launch".into(),
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn".into()),
            provider_prompt_id: None,
            prompt_payload: Some(payload),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Working, 1002)
        .unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot
        .state
        .set_draft_text(room, "VISIBLE_STEERING_PROMPT")
        .unwrap();
    let steering = snapshot.state.submit_draft(room, 1003).unwrap()[0];
    snapshot.state.begin_steering(steering, lead).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot.clone()));
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        snapshot
            .state
            .record_steering(
                steering,
                SubmissionOutcome::Uncertain {
                    message: CAUSE.into(),
                },
            )
            .unwrap();
        assert_eq!(
            snapshot
                .state
                .request(steering)
                .unwrap()
                .submission_error
                .as_deref(),
            Some(CAUSE)
        );
        snapshot.state =
            serde_json::from_slice(&serde_json::to_vec(&snapshot.state).unwrap()).unwrap();
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
        assert_no_diagnostics(&mut ui, &[CAUSE]);
        key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(ui.locals[&room].text.text, "VISIBLE_STEERING_PROMPT");
        assert_no_diagnostics(&mut ui, &[CAUSE]);
    });
    let log = capture.text();
    assert_eq!(log.matches(CAUSE).count(), 1, "{log}");
    for field in [
        "bus.request.failed".into(),
        format!("request_id={}", steering.0),
        format!("agent_id={}", agent.0),
        format!("room_id={}", room.0),
    ] {
        assert!(log.contains(&field), "missing {field}: {log}");
    }
}

#[test]
fn shared_request_classification_keeps_provider_errors_and_bus_failures_at_warning() {
    // The companion deliberate_request_outcomes_log_at_info... pins all eight
    // deliberate reasons. Provider errors are external to dashboard reliability,
    // but remain actionable warnings in request diagnostics.
    for reason in [
        "provider_error: quota",
        "provider_error: APIProviderQuotaExhaustedError: 429 You've reached your monthly usage limit for this billing cycle.",
        "reply_not_captured",
        "submission_unconfirmed",
    ] {
        let (mut ui, mut snapshot, _) = snapshot_with_terminal_reason(reason);
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
        });
        assert_no_diagnostics(&mut ui, &[reason]);
        assert!(
            capture.text().contains("WARN") && capture.text().contains("bus.request.failed"),
            "{}",
            capture.text()
        );
    }
}
