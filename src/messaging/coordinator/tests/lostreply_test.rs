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
fn unconfirmed_steering_settles_under_working_status_without_duplicate_publication() {
    let mut f = orchestrator();
    let (lead, unknown) = stop_with_unconfirmed_steering(&mut f);
    let first_unread = f.worker.state.room(f.room).unwrap().unread_count;
    f.now = f
        .request(lead)
        .pending_final
        .as_ref()
        .unwrap()
        .received_at_ms
        + crate::messaging::model::STEERING_SETTLE_MS
        + 1;
    f.status(RuntimeStatus::Working);
    f.consume();
    assert_eq!(f.reply(unknown).as_deref(), Some("Workers started."));
    assert_eq!(f.reply(lead).as_deref(), Some("Workers started."));
    assert_eq!(
        f.worker.state.room(f.room).unwrap().unread_count,
        first_unread
    );
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
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
