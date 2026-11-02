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
