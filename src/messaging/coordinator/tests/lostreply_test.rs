//! 2026-10-27: a finished MASTER answer waited on five background shells.
use super::*;

fn orchestrator() -> Fixture {
    let mut f = master();
    let mut state = f.worker.state.clone();
    let work = state.create_room("work").unwrap();
    state.bind_orchestrator(f.agent, work).unwrap();
    f.worker.save(state).unwrap();
    f.consume();
    f
}

fn settle_stop(f: &mut Fixture) {
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.now += crate::messaging::model::TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
}

#[test]
fn master_developer_answer_posts_at_stop_with_five_shells_still_running() {
    let mut f = orchestrator();
    // One inherited shell, followed by four new worker dispatches. Keep the
    // incident's shell syntax without copying private brief contents.
    let inherited = ("inherited", "sleep 3600");
    stop(&f, "old-turn", "Old answer", &[inherited]);
    f.consume();
    let lead = running(&mut f, "investigate four performance bugs", "turn-1");
    let joined = f.send("10 seconds is acceptable", false);
    f.deliver();
    f.started("turn-1", "10 seconds is acceptable");
    f.consume();
    assert_eq!(f.request(joined).group, Some(lead));
    stop(
        &f,
        "turn-1",
        "Four workers have started.",
        &[
            inherited,
            ("play", "S=/tmp/briefs; cat > $S/common.txt <<'EOF'\nBrief\nEOF\nB=/path/bus\n$B send --async --text \"$(cat $S/common.txt)\""),
            ("network", r#"S=/tmp/briefs; /path/bus send --async --text "$(cat $S/common.txt)""#),
            ("waveform", r#"S=/tmp/briefs; /path/bus send --async --text "$(cat $S/common.txt)""#),
            ("cold-open", r#"S=/tmp/briefs; /path/bus send --async --text "$(cat $S/common.txt)""#),
        ],
    );
    // Stop is authoritative even when native status remains Working because
    // its background shells are still alive. No idle poll or new prompt.
    f.consume();
    for id in [lead, joined] {
        let status = f
            .worker
            .control_message_at(f.request(id).prompt.id, f.now)
            .unwrap();
        assert_eq!(status["complete"], true, "{status}");
        assert_eq!(status["requests"][0]["stage"], "replied");
        assert_eq!(status["requests"][0]["turn_ended"], true);
        assert!(status["requests"][0]["reason"].is_null());
        assert_eq!(f.reply(id).as_deref(), Some("Four workers have started."));
    }
    // A later shell wake is a new report, never a replacement for the answer.
    f.status(RuntimeStatus::Working);
    f.started("turn-2", &notification("play"));
    f.stopped("turn-2", "The play worker finished.");
    settle_stop(&mut f);
    assert_eq!(f.reply(lead).as_deref(), Some("Four workers have started."));
    let stored = f.worker.store.load().unwrap().unwrap();
    assert_eq!(
        stored.request(joined).unwrap().phase,
        RequestPhase::Completed
    );
    let work = stored.agent(f.agent).unwrap().orchestrates.unwrap();
    assert!(stored
        .notices(work)
        .any(|notice| notice.text == "The play worker finished."));
}

#[test]
fn later_unrelated_human_prompt_cannot_delay_or_replace_the_captured_stop_answer() {
    let mut f = orchestrator();
    let lead = running(&mut f, "report the dispatch", "turn-1");
    stop(
        &f,
        "turn-1",
        "The workers are dispatched.",
        &[("dispatch", "opaque-wrapper --async")],
    );
    f.consume();
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("The workers are dispatched.")
    );
    let transcript = f.dir.join("unrelated.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type":"user", "sessionId":"session", "promptId":"unrelated",
                "promptSource":"typed", "turnOrigin":"human",
                "message":{"role":"user", "content":"another question"}
            })
        ),
    )
    .unwrap();
    f.record(
        json!({"hook_event_name":"UserPromptSubmit", "session_id":"session",
        "prompt_id":"unrelated", "prompt":"another question", "transcript_path":transcript}),
    );
    f.stopped("unrelated", "An unrelated answer.");
    settle_stop(&mut f);
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("The workers are dispatched.")
    );
    let status = f
        .worker
        .control_message_at(f.request(lead).prompt.id, f.now)
        .unwrap();
    assert_eq!(status["requests"][0]["turn_ended"], true);
}

#[test]
fn stop_text_is_delivered_even_with_background_agents_and_session_crons() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "report progress", "turn-1");
    f.record(
        json!({"hook_event_name":"Stop", "session_id":"session", "prompt_id":"turn-1",
        "last_assistant_message":"A complete answer.",
        "background_tasks":[{"id":"agent","type":"local_agent","status":"running"}],
        "session_crons":[{"id":"cron","status":"running"}]}),
    );
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("A complete answer."));
}

fn stop_with_unconfirmed_steering(f: &mut Fixture) -> (RequestId, RequestId) {
    let lead = running(f, "start the workers", "turn-1");
    let unknown = f.send("also run checks", false);
    f.deliver();
    stop(
        f,
        "turn-1",
        "Workers started.",
        &[("dispatch", "opaque-wrapper --async")],
    );
    f.consume();
    (lead, unknown)
}

#[test]
fn unconfirmed_steering_never_holds_the_captured_leads_answer() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert!(
        f.reply(unknown).is_none(),
        "its submit hook may still name another turn"
    );
    let status = f
        .worker
        .control_message_at(f.request(lead).prompt.id, f.now)
        .unwrap();
    assert_eq!(status["complete"], true);
    assert_eq!(status["requests"][0]["turn_ended"], true);
}

#[test]
fn unconfirmed_steering_stays_uncertain_while_working_without_a_borrowed_answer() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    let first_unread = f.worker.state.room(f.room).unwrap().unread_count;
    f.now = f
        .request(lead)
        .pending_final
        .as_ref()
        .unwrap()
        .received_at_ms
        + 5_000
        + 1;
    f.status(RuntimeStatus::Working);
    f.consume();
    assert!(f.reply(unknown).is_none());
    assert!(f.request(unknown).uncertain_outcome);
    assert!(f.request(unknown).submission_notice().is_some());
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(
        f.worker.state.room(f.room).unwrap().unread_count,
        first_unread
    );
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(lead)
    );
}

#[test]
fn a_same_turn_steering_hook_after_stop_delivers_without_an_idle_poll() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    f.started("turn-1", "also run checks");
    f.consume();
    assert_eq!(f.reply(unknown).as_deref(), Some("Workers started."));
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(f.worker.state.room(f.room).unwrap().unread_count, 1);
}

#[test]
fn a_provider_failure_during_steering_correlation_cannot_revoke_the_captured_answer() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    let completed = f.request(lead).completed_at_ms;
    f.record(
        json!({"hook_event_name":"StopFailure", "session_id":"session",
        "prompt_id":"turn-1", "error":"server_error", "last_assistant_message":"API failed"}),
    );
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(f.request(lead).completed_at_ms, completed);
    assert!(f.request(lead).failure_reason.is_none());
    assert_eq!(f.request(unknown).phase, RequestPhase::Abandoned);
}

#[test]
fn textless_background_progress_during_steering_correlation_cannot_erase_the_answer() {
    let mut f = orchestrator();
    let (lead, _) = stop_with_unconfirmed_steering(&mut f);
    f.record(
        json!({"hook_event_name":"Stop", "session_id":"session", "prompt_id":"turn-1",
        "background_tasks":[{"type":"local_agent"}]}),
    );
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
}

#[test]
fn a_late_same_turn_cross_room_member_publishes_only_in_its_new_room() {
    let mut f = orchestrator();
    let lead = running(&mut f, "help the workers", "turn-1");
    let work = f.worker.state.agent(f.agent).unwrap().orchestrates.unwrap();
    let mut state = f.worker.state.clone();
    let blocked = state
        .create_agent(work, "blocked", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    f.bind_dialog_pane(blocked);
    // Two coherent observations establish the blocked-dialog episode.
    for _ in 0..2 {
        f.worker
            .notify_dialogs(vec![(blocked, Some("codex:pending_question".into()))])
            .unwrap();
    }
    f.deliver();
    let notice = *f.worker.state.group_members(lead).last().unwrap();
    let payload = f.typed().last().unwrap().0.clone();
    let master_before = f.worker.state.room(f.room).unwrap().unread_count;
    let work_before = f.worker.state.room(work).unwrap().unread_count;
    stop(
        &f,
        "turn-1",
        "Help is underway.",
        &[("dispatch", "opaque-wrapper")],
    );
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Help is underway."));
    assert_eq!(
        f.worker.state.room(f.room).unwrap().unread_count,
        master_before + 1
    );
    assert_eq!(f.worker.state.room(work).unwrap().unread_count, work_before);
    f.started("turn-1", &payload);
    f.consume();
    assert_eq!(f.reply(notice).as_deref(), Some("Help is underway."));
    assert_eq!(
        f.worker.state.room(f.room).unwrap().unread_count,
        master_before + 1
    );
    assert_eq!(
        f.worker.state.room(work).unwrap().unread_count,
        work_before + 1
    );
}

#[test]
fn recovering_a_correlation_anchor_preserves_its_completed_answer() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    let mut state = f.worker.state.clone();
    state.invalidate_agent_session(f.agent).unwrap();
    state
        .observe_status(f.agent, RuntimeStatus::Unavailable, f.now)
        .unwrap();
    state.recover_idle_request(lead, f.now).unwrap();
    f.worker.save(state).unwrap();
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(f.request(unknown).phase, RequestPhase::Abandoned);
}

fn reopen(f: &mut Fixture) {
    // Drop the previous worker's exclusive state lease before reopening it.
    let swap = crate::utils::test_temp::unique_temp_path("bus-lostreply-reopen");
    let old = std::mem::replace(
        &mut f.worker,
        Worker::open(swap.clone(), Box::new(Native(f.native.clone()))).unwrap(),
    );
    drop(old);
    f.worker = Worker::open(f.dir.clone(), Box::new(Native(f.native.clone()))).unwrap();
    std::fs::remove_dir_all(swap).unwrap();
}

#[test]
fn identical_same_turn_stop_with_changed_shells_has_one_master_publication() {
    // Adopt the reviewer's scratch callback sequence, including a coordinator
    // reopen: changing the shell inventory changes the callback envelope.
    for restart in [false, true] {
        let mut f = orchestrator();
        let lead = running(&mut f, "report the dispatch", "turn-1");
        let answer = "Identical captured answer must appear once.";
        stop(&f, "turn-1", answer, &[("work", "opaque-wrapper --async")]);
        f.consume();
        assert_eq!(f.reply(lead).as_deref(), Some(answer));
        let unread = f.worker.state.room(f.room).unwrap().unread_count;
        if restart {
            reopen(&mut f);
        }
        stop(&f, "turn-1", answer, &[]);
        f.consume();
        assert_eq!(f.reply(lead).as_deref(), Some(answer));
        assert_eq!(
            f.worker
                .state
                .notices(f.room)
                .filter(|n| n.text == answer)
                .count(),
            0,
            "the completed request already published this answer; restart={restart}"
        );
        assert_eq!(f.worker.state.room(f.room).unwrap().unread_count, unread);
    }
}

#[test]
fn changed_same_turn_stop_during_steering_correlation_remains_a_visible_followup() {
    // Adopt the reviewer's steering-followup probe, then pin immutability and
    // replay behavior across another turn and coordinator reopen.
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(lead)
    );
    assert!(!f.request(unknown).trusted_start_bound);
    let unread = f.worker.state.room(f.room).unwrap().unread_count;
    stop(&f, "turn-1", "Workers started.", &[]);
    f.consume();
    assert_eq!(f.worker.state.room(f.room).unwrap().unread_count, unread);
    assert_eq!(f.worker.state.notices(f.room).count(), 0);
    let followup = "All background tests passed.";
    stop(&f, "turn-1", followup, &[]);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(
        f.worker
            .state
            .notices(f.room)
            .filter(|n| n.text == followup)
            .count(),
        1,
        "changed captured Stop must remain visible while the completed lead anchors steering"
    );
    stop(&f, "turn-1", followup, &[("other", "opaque-wrapper")]);
    f.consume();
    f.started("turn-1", "also run checks");
    f.consume();
    assert_eq!(f.reply(unknown).as_deref(), Some("Workers started."));
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
    f.started("unrelated", "another question typed directly");
    f.stopped("unrelated", "An unrelated answer.");
    f.consume();
    reopen(&mut f);
    stop(&f, "turn-1", followup, &[("replay", "opaque-wrapper")]);
    f.consume();
    assert_eq!(
        f.worker
            .state
            .notices(f.room)
            .filter(|n| n.text == followup)
            .count(),
        1
    );
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(f.reply(unknown).as_deref(), Some("Workers started."));
}

#[test]
fn identical_text_in_a_new_owned_turn_still_gets_its_own_reply() {
    let mut f = orchestrator();
    let lead = running(&mut f, "first question", "turn-1");
    stop(&f, "turn-1", "Done.", &[("work", "opaque-wrapper")]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    let next = running(&mut f, "second question", "turn-2");
    stop(&f, "turn-2", "Done.", &[]);
    f.consume();
    for id in [lead, next] {
        assert_eq!(f.reply(id).as_deref(), Some("Done."));
    }
    assert_eq!(f.worker.state.room(f.room).unwrap().unread_count, 2);
    assert_eq!(f.worker.state.notices(f.room).count(), 0);
}

#[test]
fn a_foreign_turn_stop_during_correlation_cannot_publish_a_master_followup() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    stop(
        &f,
        "foreign-turn",
        "This does not answer the MASTER request.",
        &[],
    );
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert!(f.reply(unknown).is_none());
    assert_eq!(f.worker.state.notices(f.room).count(), 0);
    assert_eq!(f.worker.state.room(f.room).unwrap().unread_count, 1);
}

#[test]
fn a_textless_stop_with_a_request_started_shell_takes_the_wake_turns_answer() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    f.consume();
    let lead = running(&mut f, "run the tests", "turn-1");
    stop(&f, "turn-1", "", &[("tests", "cargo test")]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.started("turn-2", &notification("tests"));
    f.consume();
    f.status(RuntimeStatus::Working);
    f.stopped("turn-2", "All tests passed.");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("All tests passed."));
}

#[test]
fn a_steering_member_that_starts_its_own_turn_after_the_lead_published_gets_that_turns_answer() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    let first_unread = f.worker.state.room(f.room).unwrap().unread_count;
    f.started("turn-2", "also run checks");
    f.consume();
    f.stopped("turn-2", "Checks ran.");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(f.reply(unknown).as_deref(), Some("Checks ran."));
    assert_eq!(
        f.worker.state.room(f.room).unwrap().unread_count,
        first_unread + 1
    );
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
}

#[test]
fn an_ended_unclaimed_steering_request_surfaces_uncertainty_after_reopening() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    let first_unread = f.worker.state.room(f.room).unwrap().unread_count;
    let dir = f.dir.clone();
    let native = f.native.clone();
    let worker = std::mem::replace(
        &mut f.worker,
        Worker::open(
            crate::utils::test_temp::unique_temp_path("bus-anchor-swap"),
            Box::new(Native(native.clone())),
        )
        .unwrap(),
    );
    let swap = f.worker.data_dir.clone();
    drop(worker);
    f.worker = Worker::open(dir, Box::new(Native(native))).unwrap();
    let _ = std::fs::remove_dir_all(swap);
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    f.now = f
        .request(lead)
        .pending_final
        .as_ref()
        .unwrap()
        .received_at_ms
        + 5_000
        + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert!(f.reply(unknown).is_none());
    assert_eq!(f.request(unknown).phase, RequestPhase::Abandoned);
    assert_eq!(
        f.request(unknown).failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(f.request(unknown).uncertain_outcome);
    assert!(f.request(unknown).submission_notice().is_some());
    assert_eq!(
        f.worker.state.room(f.room).unwrap().unread_count,
        first_unread
    );
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
}

#[test]
fn a_steered_turn_whose_submit_hook_trails_the_window_keeps_its_own_answer_while_working() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "write the parser", "turn-1");
    let correction = f.send("also add tests", false);
    f.deliver();
    assert_eq!(f.request(correction).group, Some(lead));
    f.stopped("turn-1", "parser written");
    f.consume();
    f.now = f
        .request(lead)
        .pending_final
        .as_ref()
        .unwrap()
        .received_at_ms
        + 5_000
        + 1;
    f.status(RuntimeStatus::Working);
    f.consume();
    f.started("turn-2", "also add tests");
    f.stopped("turn-2", "parser and tests written");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("parser written"));
    assert_eq!(
        f.reply(correction).as_deref(),
        Some("parser and tests written")
    );
}

#[test]
fn an_idle_poll_never_lends_the_previous_answer_to_unclaimed_steering() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    f.status(RuntimeStatus::Idle);
    f.consume();
    let status = f
        .worker
        .control_message_at(f.request(unknown).prompt.id, f.now)
        .unwrap();
    assert_eq!(status["complete"], true, "{status}");
    assert_eq!(status["requests"][0]["stage"], "abandoned", "{status}");
    assert_eq!(status["requests"][0]["reason"], "submission_unconfirmed");
    assert_eq!(status["requests"][0]["uncertain_outcome"], true);
    assert!(status["requests"][0]["reply"].is_null());
    assert!(f.request(unknown).submission_notice().is_some());
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
    assert_eq!(f.worker.state.notices(f.room).count(), 0);
}

#[test]
fn a_recorded_turn_claims_steering_without_borrowing_another_turns_answer() {
    use crate::messaging::model::RecordedTurnEnd;
    for same_turn in [false, true] {
        let mut f = orchestrator();
        let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
        f.status(RuntimeStatus::Idle);
        let prompts = [lead, unknown]
            .map(|id| f.request(id).submitted_payload.clone().unwrap())
            .to_vec();
        let mut state = f.worker.state.clone();
        assert!(state.settle_unclaimed_steering(f.agent, f.now, |request| {
            let proves = if same_turn {
                request.id == lead
            } else {
                request.id == unknown
            };
            proves.then(|| RecordedTurnEnd::Final {
                turn_id: if same_turn { "turn-1" } else { "turn-2" }.into(),
                text: if same_turn {
                    "Workers started."
                } else {
                    "Checks ran."
                }
                .into(),
                prompts: prompts.clone(),
            })
        }));
        f.worker.save(state).unwrap();
        assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
        assert_eq!(
            f.reply(unknown).as_deref(),
            Some(if same_turn {
                "Workers started."
            } else {
                "Checks ran."
            })
        );
        assert!(!f.request(unknown).uncertain_outcome);
        assert!(f.request(unknown).trusted_start_bound);
        assert_eq!(
            f.worker.state.room(f.room).unwrap().unread_count,
            if same_turn { 1 } else { 2 }
        );
        assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
    }
}

#[test]
fn a_changed_late_master_final_is_visible_while_new_work_awaits_its_answer() {
    for next_is_bound in [true, false] {
        let mut f = orchestrator();
        let lead = running(&mut f, "start work", "turn-1");
        stop(
            &f,
            "turn-1",
            "Workers started.",
            &[("work", "opaque-wrapper")],
        );
        f.consume();
        f.status(RuntimeStatus::Idle);
        let next = f.send("new question", false);
        f.deliver();
        if next_is_bound {
            f.started("turn-2", "new question");
            f.consume();
        }
        stop(&f, "turn-1", "The background tests passed.", &[]);
        f.consume();
        assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
        assert!(f.reply(next).is_none());
        assert_eq!(
            f.worker.state.notices(f.room).filter(|n| n.text == "The background tests passed.").count(),
            1,
            "new work must not hide the completed turn's changed result; next_is_bound={next_is_bound}"
        );
        if !next_is_bound {
            f.started("turn-2", "new question");
        }
        stop(&f, "turn-2", "New answer.", &[]);
        f.consume();
        assert_eq!(f.reply(next).as_deref(), Some("New answer."));
        assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    }
}

#[test]
fn a_later_recorded_turn_answers_every_steering_member_it_claims() {
    use crate::messaging::model::RecordedTurnEnd;
    for visible_to_all in [true, false] {
        let mut f = Fixture::new(Provider::Codex);
        let lead = running(&mut f, "write the parser", "turn-1");
        let checks = f.send("also run checks", false);
        f.deliver();
        let docs = f.send("also update docs", false);
        f.deliver();
        f.stopped("turn-1", "Parser written.");
        f.consume();
        assert_eq!(f.reply(lead).as_deref(), Some("Parser written."));
        for id in [checks, docs] {
            assert_eq!(f.request(id).group, Some(lead));
            assert!(!f.request(id).trusted_start_bound);
        }
        f.status(RuntimeStatus::Idle);
        let prompts = [checks, docs]
            .map(|id| f.request(id).submitted_payload.clone().unwrap())
            .to_vec();
        let mut state = f.worker.state.clone();
        assert!(state.settle_unclaimed_steering(f.agent, f.now, |request| {
            (request.id == docs || (visible_to_all && request.id == checks)).then(|| {
                RecordedTurnEnd::Final {
                    turn_id: "turn-2".into(),
                    text: "Checks and docs finished.".into(),
                    prompts: prompts.clone(),
                }
            })
        }));
        f.worker.save(state).unwrap();
        for id in [checks, docs] {
            assert_eq!(f.reply(id).as_deref(), Some("Checks and docs finished."),
            "every member proved by the same later recorded turn must receive its answer; member {id:?}");
            assert_eq!(f.request(id).phase, RequestPhase::Completed);
            assert!(!f.request(id).uncertain_outcome);
        }
        assert_eq!(f.reply(lead).as_deref(), Some("Parser written."));
        assert_eq!(
            f.worker.state.room(f.room).unwrap().unread_count,
            2,
            "the later shared answer publishes once in the room"
        );
    }
}

#[test]
fn a_new_silent_request_never_owns_a_previous_requests_shell_after_reopen() {
    for (restart, retry_startup) in [(false, false), (true, false), (true, true)] {
        let mut f = orchestrator();
        let old = running(&mut f, "start a background service", "old-turn");
        let service = ("persistent-old-service", "old-background-service");
        stop(&f, "old-turn", "The old service is running.", &[service]);
        f.consume();
        assert_eq!(f.reply(old).as_deref(), Some("The old service is running."));
        if restart {
            reopen(&mut f);
        }
        if retry_startup {
            f.record(json!({"hook_event_name":"SessionStart", "session_id":"session", "source":"startup", "retry":true}));
            f.consume();
        }
        f.status(RuntimeStatus::Idle);
        let silent = running(
            &mut f,
            "do not reply; start no background work",
            "silent-turn",
        );
        stop(&f, "silent-turn", "", &[service]);
        f.consume();
        let status = f
            .worker
            .control_message_at(f.request(silent).prompt.id, f.now)
            .unwrap();
        assert_eq!(status["complete"], true, "restart={restart}: {status}");
        assert_eq!(f.reply(silent).as_deref(), Some(""));
        assert!(!f.request(silent).awaiting_background);
        assert_eq!(f.reply(old).as_deref(), Some("The old service is running."));
        assert_eq!(f.worker.state.notices(f.room).count(), 0);
    }
}

#[test]
fn a_request_owned_textless_wait_keeps_its_shell_ownership_after_reopen() {
    let mut f = orchestrator();
    let lead = running(&mut f, "run tests and report their result", "turn-1");
    stop(&f, "turn-1", "", &[("tests", "cargo test")]);
    f.consume();
    assert!(f.request(lead).awaiting_background);
    assert!(f.reply(lead).is_none());
    reopen(&mut f);
    f.status(RuntimeStatus::Idle);
    // A different envelope reports the same running task after restart.
    stop(&f, "turn-1", "", &[("tests", "cargo test (still running)")]);
    f.consume();
    assert!(f.request(lead).awaiting_background);
    assert!(f.reply(lead).is_none());
    f.started("wake", &notification("tests"));
    f.stopped("wake", "All tests passed.");
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("All tests passed."));
}

#[test]
fn an_unknown_shell_census_is_not_proof_of_request_ownership() {
    // The session was attested before this process, but no startup inventory
    // or earlier Stop was saved. Existing shells have unknown ownership.
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "do not reply", "turn-1");
    stop(&f, "turn-1", "", &[("unknown-service", "daemon")]);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some(""));
    assert!(!f.request(lead).awaiting_background);
}
