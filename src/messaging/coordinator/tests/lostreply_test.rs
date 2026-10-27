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
