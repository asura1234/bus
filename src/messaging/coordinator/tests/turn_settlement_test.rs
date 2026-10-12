use super::*;

// The callback spool gets a bounded grace period after the first settled poll.
const CALLBACK_GRACE_MS: u64 = crate::messaging::model::TURN_SETTLE_MS;

#[test]
fn failed_provider_turn_closes_its_group_and_releases_the_queue() {
    for provider in [Provider::Cursor, Provider::ClaudeCode] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "review this change", "turn-1");
        let joined = fixture.send("check the tests too", false);
        fixture.deliver();
        fixture.started("turn-1", "check the tests too");
        fixture.consume();
        let next = fixture.send("review the next change", true);
        let failed = match provider {
            Provider::Cursor => {
                json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-1","status":"error"})
            }
            _ => {
                json!({"hook_event_name":"StopFailure","session_id":"session","prompt_id":"turn-1"})
            }
        };
        fixture.record(failed);
        fixture.consume();
        for id in [lead, joined] {
            assert_eq!(
                fixture.request(id).phase,
                RequestPhase::Abandoned,
                "{provider:?}"
            );
            assert!(fixture.request(id).completed_at_ms.is_some());
            assert!(fixture.reply(id).is_none());
        }
        assert!(fixture.error().is_some());
        assert_eq!(
            fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .current_request,
            None
        );
        fixture.status(RuntimeStatus::Idle);
        fixture.deliver();
        assert_eq!(fixture.request(next).group, None);
        assert_eq!(
            fixture.typed().last().unwrap(),
            &("review the next change".into(), false)
        );
    }
}

#[test]
fn transcript_unmatched_turn_closes_without_a_reply_and_unblocks_next_request() {
    for provider in [Provider::Cursor, Provider::ClaudeCode] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "review this change", "turn-1");
        let next = fixture.send("review the next change", true);
        if provider == Provider::Cursor {
            let transcript = fixture.dir.join("incomplete.jsonl");
            std::fs::write(&transcript, concat!(
                "{\"role\":\"user\"}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Checking.\"},{\"type\":\"tool_use\"}]}}\n"
            )).unwrap();
            fixture.record(json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn-1","text":"Checking.Ready","transcript_path":transcript}));
            fixture.record(json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-1","status":"completed"}));
        } else {
            // A completed Claude hook with no captured assistant message.
            fixture.record(
                json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1"}),
            );
        }
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
        fixture.now += CALLBACK_GRACE_MS;
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        assert_eq!(
            fixture.request(lead).phase,
            RequestPhase::Abandoned,
            "{provider:?}"
        );
        assert!(fixture
            .worker
            .state
            .room(fixture.room)
            .unwrap()
            .latest_replies
            .is_empty());
        fixture.deliver();
        assert_eq!(fixture.request(next).group, None);
        assert_eq!(
            fixture.typed().last().unwrap(),
            &("review the next change".into(), false)
        );
        let persisted = fixture.worker.store.load().unwrap().unwrap();
        assert_eq!(
            persisted.request(lead).unwrap().phase,
            RequestPhase::Abandoned
        );
    }
}

#[test]
fn ended_codex_turn_closes_existing_joins_and_cannot_collect_new_joins() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    let mut joined = Vec::new();
    for index in 0..8 {
        let text = format!("correction {index}");
        joined.push(fixture.send(&text, false));
        fixture.deliver();
        fixture.started("turn-1", &text);
        fixture.consume();
    }
    fixture.status(RuntimeStatus::Idle);
    // Another activity can make the terminal Working again before reply capture.
    fixture.status(RuntimeStatus::Working);
    let next = fixture.send("an independent task", false);
    fixture.deliver();
    assert_eq!(
        fixture.typed().len(),
        9,
        "a finished request must not take another join"
    );
    assert_eq!(fixture.request(next).phase, RequestPhase::Queued);
    assert_eq!(fixture.request(next).group, None);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for id in std::iter::once(lead).chain(joined) {
        assert_eq!(fixture.request(id).phase, RequestPhase::Abandoned);
        assert!(fixture.request(id).completed_at_ms.is_some());
        assert!(fixture.reply(id).is_none());
    }
    fixture.deliver();
    assert_eq!(
        fixture.typed().last().unwrap(),
        &("an independent task".into(), false)
    );
}

#[test]
fn a_final_arriving_during_the_idle_grace_period_still_completes() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS - 2;
    fixture.status(RuntimeStatus::Idle);
    fixture.stopped("turn-1", "implemented");
    fixture.consume();
    assert_eq!(fixture.reply(lead).as_deref(), Some("implemented"));
}

#[test]
fn a_visible_choice_panel_does_not_close_an_idle_codex_request() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    let mut state = fixture.worker.state.clone();
    state.observe_dialog(fixture.agent, true).unwrap();
    fixture.worker.save(state).unwrap();
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
}

#[test]
fn explicit_background_work_can_resume_after_the_callback_grace_period() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1","background_tasks":[{"type":"agent"}]}));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS * 2;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.started(
        "turn-2",
        "<task-notification>background agent finished</task-notification>",
    );
    fixture.stopped("turn-2", "implemented");
    fixture.consume();
    assert_eq!(fixture.reply(lead).as_deref(), Some("implemented"));
}

#[test]
fn a_final_spooled_at_the_deadline_is_read_before_the_request_closes() {
    let mut fixture = Fixture::new(Provider::Cursor);
    let lead = running(&mut fixture, "review the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.stopped("turn-1", "Ready");
    fixture.consume();
    assert_eq!(fixture.reply(lead).as_deref(), Some("Ready"));
}

#[test]
fn a_failure_from_another_turn_does_not_release_the_current_owner() {
    let mut fixture = Fixture::new(Provider::Cursor);
    let lead = running(&mut fixture, "review the change", "turn-1");
    fixture.record(json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"old-turn","status":"error"}));
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        Some(lead)
    );
    assert_eq!(fixture.error(), None);
}

#[test]
fn a_turn_with_status_evidence_but_no_start_or_reply_hook_also_closes() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = fixture.send("implement the change", false);
    fixture.deliver();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    assert!(!fixture.request(lead).uncertain_outcome);
    assert!(!fixture.request(lead).trusted_start_bound);
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    // The deadline is durable, including when a coordinator reloads its state.
    fixture.worker.state = fixture.worker.store.load().unwrap().unwrap();
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Abandoned);
    assert!(fixture
        .worker
        .state
        .room(fixture.room)
        .unwrap()
        .latest_replies
        .is_empty());
}

#[test]
fn a_codex_turn_that_ended_on_a_provider_error_reports_that_error() {
    // Live: Codex fires no Stop hook when "Selected model is at capacity"
    // ends a turn; only its rollout's task_complete event carries the error.
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-12T00-00-00-abc.jsonl");
    std::fs::write(
        &rollout,
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1","last_agent_message":null,
            "error":{"message":"Selected model is at capacity. Please try a different model.","codex_error_info":"server_overloaded"}}})
        .to_string() + "\n",
    )
    .unwrap();
    let lead = fixture.send("implement the change", false);
    fixture.deliver();
    fixture.record(json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn-1","prompt":"implement the change","transcript_path":rollout}));
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    let request = fixture.request(lead);
    assert_eq!(request.phase, RequestPhase::Abandoned);
    let expected = "provider_error: Selected model is at capacity. Please try a different model. (server_overloaded)";
    assert_eq!(request.failure_reason.as_deref(), Some(expected));
    assert!(fixture
        .error()
        .unwrap()
        .contains("Selected model is at capacity"));
}

#[test]
fn a_turn_closed_without_a_reply_or_provider_error_says_so_in_its_reason() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    let request = fixture.request(lead);
    assert_eq!(request.phase, RequestPhase::Abandoned);
    assert_eq!(
        request.failure_reason.as_deref(),
        Some("reply_not_captured")
    );
}

fn claude_stop(turn: &str, text: &str, shells: &[&str]) -> Value {
    // Shape measured from Claude Code 2.1.296: each running background shell
    // is listed, and each one that finishes wakes a turn whose Stop drops it.
    let tasks: Vec<_> = shells
        .iter()
        .map(|id| json!({"id":id,"type":"shell","status":"running","description":"Run","command":"sleep 45"}))
        .collect();
    json!({"hook_event_name":"Stop","session_id":"session","prompt_id":turn,"last_assistant_message":text,"background_tasks":tasks,"session_crons":[]})
}

fn claude_startup() -> Value {
    json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"})
}

/// Regression: a Claude worker that ended its turn while shells it started
/// still ran ("4 shells still running") had its interim text recorded as the
/// final reply, so the orchestrator's `send --async` returned early.
#[test]
fn a_claude_turn_waiting_on_shells_it_started_keeps_its_request_open() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(&mut fixture, "run the tests", "turn-1");
    fixture.record(claude_stop(
        "turn-1",
        "Waiting on the background shells.",
        &["b1", "b2"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    assert!(fixture.reply(lead).is_none());
    // The first shell finishing wakes a turn; the other still runs.
    fixture.started(
        "turn-2",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.record(claude_stop("turn-2", "One shell finished.", &["b2"]));
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.started(
        "turn-3",
        "<task-notification>b2 completed</task-notification>",
    );
    fixture.consume();
    fixture.record(claude_stop("turn-3", "Both test runs passed.", &[]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Both test runs passed.")
    );
}

/// A shell left running before the request (say a hung `find /`) is listed by
/// every later Stop and must not hold this or any later reply.
#[test]
fn a_claude_shell_left_running_before_the_request_does_not_hold_its_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let first = running(&mut fixture, "look around", "turn-1");
    fixture.record(claude_stop("turn-1", "Looked around.", &[]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(first).phase, RequestPhase::Completed);
    // A turn of the agent's own leaves a shell running.
    fixture.record(claude_stop("own-turn", "Started a search.", &["stale"]));
    fixture.consume();
    let lead = running(&mut fixture, "review this change", "turn-2");
    fixture.record(claude_stop("turn-2", "Reviewed.", &["stale"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(fixture.reply(lead).as_deref(), Some("Reviewed."));
}

/// After a Bus restart the shells running before the request are unknown, so
/// none of them holds a reply.
#[test]
fn a_claude_reply_is_not_held_by_shells_of_unknown_age() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "review this change", "turn-1");
    fixture.record(claude_stop("turn-1", "Reviewed.", &["unknown"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
}

#[test]
fn a_joined_codex_request_reports_the_provider_error_that_ended_its_turn() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    let joined = fixture.send("check the tests too", false);
    fixture.deliver();
    let rollout = fixture.dir.join("rollout-joined.jsonl");
    std::fs::write(&rollout, json!({"type":"event_msg","payload":{
        "type":"task_complete","turn_id":"turn-1","error":{"message":"Selected model is at capacity."}
    }}).to_string()).unwrap();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"check the tests too","transcript_path":rollout}),
    );
    fixture.consume();
    assert_eq!(fixture.request(joined).group, Some(lead));
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(joined).phase, RequestPhase::Abandoned);
    let message = fixture.request(joined).prompt.id;
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(
        status["requests"][0]["reason"],
        "provider_error: Selected model is at capacity."
    );
}
