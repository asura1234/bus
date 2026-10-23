use super::*;

// The callback spool gets a bounded grace period after the first settled poll.
const CALLBACK_GRACE_MS: u64 = crate::messaging::model::TURN_SETTLE_MS;
const HELD_REPLY_GRACE_MS: u64 = crate::messaging::model::HELD_REPLY_SETTLE_MS;

#[test]
fn reply_capture_diagnostics_identify_every_abandoned_group_member() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let mut fixture = Fixture::new(Provider::ClaudeCode);
        let lead = running(&mut fixture, "PRIVATE_PROMPT", "turn-1");
        let joined = fixture.send("PRIVATE_STEERING", false);
        fixture.deliver();
        fixture.started("turn-1", "PRIVATE_STEERING");
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        fixture.now += CALLBACK_GRACE_MS;
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        let log = capture.text();
        for id in [lead, joined] {
            let row = log
                .lines()
                .find(|line| {
                    line.contains("bus.message.recovered")
                        && line.contains("reason=\"reply_not_captured\"")
                        && line.contains(&format!("request_id={} ", id.0))
                })
                .expect("every closed member must be diagnosable after deletion");
            for field in [
                format!("lead_id={}", lead.0),
                "reason=\"reply_not_captured\"".into(),
                "session_id=Some(\"session\")".into(),
                "turn_id=Some(\"turn-1\")".into(),
                "start_bound=true".into(),
            ] {
                assert!(row.contains(&field), "missing {field}: {row}");
            }
        }
        assert!(!log.contains("PRIVATE"), "{log}");
    });
}

#[test]
fn reply_capture_diagnostics_record_a_settlement_once_after_a_failed_save_is_retried() {
    for reason in ["reply_not_captured", "agent_unavailable", "provider_error"] {
        let mut fixture = Fixture::new(Provider::Codex);
        let lead = running(&mut fixture, "PRIVATE_PROMPT", "turn-1");
        let follower = fixture.send("PRIVATE_STEERING", false);
        fixture.deliver();
        fixture.started("turn-1", "PRIVATE_STEERING");
        fixture.consume();
        if reason == "provider_error" {
            let rollout = fixture.dir.join("rollout-error.jsonl");
            std::fs::write(
                &rollout,
                json!({"type":"event_msg","payload":{
                    "type":"task_complete","turn_id":"turn-1","error":{"message":"PRIVATE_ERROR"}
                }})
                .to_string(),
            )
            .unwrap();
            fixture.record(
                json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
                "turn_id":"turn-1","prompt":"PRIVATE_STEERING","transcript_path":rollout}),
            );
            fixture.consume();
        }
        fixture.status(RuntimeStatus::Idle);
        let status = if reason == "agent_unavailable" {
            RuntimeStatus::Unavailable
        } else {
            RuntimeStatus::Idle
        };
        fixture.status(status);
        fixture.now += CALLBACK_GRACE_MS;
        fixture.status(status);
        fixture
            .worker
            .store
            .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            assert!(fixture.worker.settle_requests().is_err());
            assert!(!capture.text().contains("bus.message.recovered"));
            assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
            fixture.worker.settle_requests().unwrap();
            fixture.worker.settle_requests().unwrap();
        });
        let log = capture.text();
        for id in [lead, follower] {
            let rows: Vec<_> = log
                .lines()
                .filter(|line| {
                    line.contains("bus.message.recovered")
                        && line.contains(&format!("request_id={} ", id.0))
                })
                .collect();
            assert_eq!(rows.len(), 1, "{reason}: {log}");
            assert!(
                rows[0].contains(&format!("reason=\"{reason}\"")),
                "{reason}: {}",
                rows[0]
            );
            assert_eq!(fixture.request(id).phase, RequestPhase::Abandoned);
        }
        assert!(
            !log.contains("bus.message.no_reply") && !log.contains("PRIVATE"),
            "{log}"
        );
    }
}

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
    state
        .observe_dialog(
            fixture.agent,
            true,
            state.agent(fixture.agent).unwrap().observed_at_ms,
        )
        .unwrap();
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

/// Live (2026-10-13, Claude Code 2.1.296): "API Error: Connection lost
/// mid-response" ended a worker's turn with a StopFailure hook, and Bus closed
/// the request with no reason. The payload names the error, as measured.
#[test]
fn a_claude_turn_ended_by_an_api_error_reports_that_error() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "run the e2e", "turn-1");
    let joined = fixture.send("check the tests too", false);
    fixture.deliver();
    fixture.started("turn-1", "check the tests too");
    fixture.consume();
    fixture.record(json!({"hook_event_name":"StopFailure","session_id":"session","prompt_id":"turn-1",
        "error":"server_error",
        "last_assistant_message":"API Error: Connection lost mid-response. The response above may be incomplete."}));
    fixture.consume();
    let expected = "provider_error: API Error: Connection lost mid-response. The response above may be incomplete. (server_error)";
    for id in [lead, joined] {
        assert_eq!(fixture.request(id).phase, RequestPhase::Abandoned);
        assert_eq!(
            fixture.request(id).failure_reason.as_deref(),
            Some(expected)
        );
    }
    assert!(fixture
        .error()
        .unwrap()
        .contains("Connection lost mid-response"));
    let message = fixture.request(joined).prompt.id;
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(status["requests"][0]["reason"], expected);
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

pub(super) fn claude_stop(turn: &str, text: &str, shells: &[&str]) -> Value {
    // Shape measured from Claude Code 2.1.296: each running background shell
    // is listed, and each one that finishes wakes a turn whose Stop drops it.
    let tasks: Vec<_> = shells
        .iter()
        .map(|id| json!({"id":id,"type":"shell","status":"running","description":"Run","command":"sleep 45"}))
        .collect();
    json!({"hook_event_name":"Stop","session_id":"session","prompt_id":turn,"last_assistant_message":text,"background_tasks":tasks,"session_crons":[]})
}

pub(super) fn claude_startup() -> Value {
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

/// A follower polling message status between a reply-less turn end and its
/// settlement must not take the turn as done: Bus attaches the provider error
/// (or `reply_not_captured`) only after the settle window. Live e2e: `send
/// --async` exited 0 at Idle, 5 s before the provider error was attached.
#[test]
fn a_reply_less_ended_turn_reports_settling_until_its_reason_is_attached() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    let joined = fixture.send("check the tests too", false);
    fixture.deliver();
    let rollout = fixture.dir.join("rollout-settling.jsonl");
    std::fs::write(&rollout, json!({"type":"event_msg","payload":{
        "type":"task_complete","turn_id":"turn-1","error":{"message":"Selected model is at capacity."}
    }}).to_string()).unwrap();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"check the tests too","transcript_path":rollout}),
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for request in [lead, joined] {
        let message = fixture.request(request).prompt.id;
        let status = fixture
            .worker
            .control_message_at(message, fixture.now)
            .unwrap();
        assert_eq!(status["requests"][0]["turn_ended"], true, "{status}");
        assert_eq!(status["requests"][0]["settling"], true, "{status}");
    }
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for request in [lead, joined] {
        let message = fixture.request(request).prompt.id;
        let status = fixture
            .worker
            .control_message_at(message, fixture.now)
            .unwrap();
        assert_eq!(status["requests"][0]["stage"], "abandoned", "{status}");
        assert_eq!(status["requests"][0]["settling"], false, "{status}");
    }
}

/// An ended turn whose agent then went unavailable is settling: async
/// waiters, which finish only on a complete message, wait for its reason.
#[test]
fn an_ended_turn_whose_agent_is_unavailable_is_settling() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.status(RuntimeStatus::Unavailable);
    let message = fixture.request(lead).prompt.id;
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(status["requests"][0]["turn_ended"], true, "{status}");
    assert_eq!(status["requests"][0]["settling"], true, "{status}");
}

/// Review of #128: a provider that exits after its turn ended left the
/// request open forever, so `send --async` / `wait --async` never exited.
/// Once the settle window passes from when the agent went unavailable, the
/// request closes as `agent_unavailable` and the waiters exit 1.
#[test]
fn an_ended_turn_on_an_unavailable_agent_eventually_releases_async_waiters() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    // Ended long ago; the window counts from going unavailable.
    fixture.now += CALLBACK_GRACE_MS * 10;
    fixture.status(RuntimeStatus::Unavailable);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Unavailable);
    fixture.consume();
    let message = fixture.request(lead).prompt.id;
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(status["complete"], true, "{status}");
    assert_eq!(status["requests"][0]["stage"], "abandoned", "{status}");
    assert_eq!(
        status["requests"][0]["reason"], "agent_unavailable",
        "{status}"
    );
    assert_eq!(status["requests"][0]["settling"], false, "{status}");
}

/// An agent that is unavailable only while it reattaches (a Bus restart)
/// and is Idle again within the window settles as an idle agent would.
#[test]
fn an_ended_turn_whose_agent_reattaches_within_the_window_settles_as_idle() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.status(RuntimeStatus::Unavailable);
    fixture.now += CALLBACK_GRACE_MS - 10;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
}

/// A reply held for the steering settle window still completes its group
/// when the agent goes unavailable instead of Idle.
#[test]
fn a_held_group_reply_completes_when_the_agent_goes_unavailable() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    fixture.stopped("turn-1", "parser and tests written");
    fixture.consume();
    fixture.status(RuntimeStatus::Unavailable);
    assert_eq!(fixture.request(correction).phase, RequestPhase::Active);
    fixture.now =
        crate::messaging::storage::io::now_ms() + crate::messaging::model::STEERING_SETTLE_MS;
    fixture.status(RuntimeStatus::Unavailable);
    fixture.consume();
    for id in [lead, correction] {
        assert_eq!(fixture.request(id).phase, RequestPhase::Completed);
        assert_eq!(
            fixture.reply(id).as_deref(),
            Some("parser and tests written")
        );
    }
}

const SHELL_HOLD_MS: u64 = 15 * 60 * 1000;
use crate::messaging::model::QUEUED_STALL_MS;

/// Holds `fixture`'s request on one shell its turn started, as Claude reports
/// it ("Waiting on the build."), and returns the request.
fn held_on_a_started_shell(fixture: &mut Fixture) -> RequestId {
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(fixture, "build it", "turn-1");
    fixture.record(claude_stop("turn-1", "Waiting on the build.", &["b1"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    lead
}

/// A shell the turn started that never exits (a dev server, say) must not hold
/// the request and the agent's queue forever: at the cap (15 minutes by
/// default) the latest interim text is the reply.
#[test]
fn a_claude_reply_held_past_the_shell_cap_is_delivered_as_its_interim_text() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let next = fixture.send("review the next change", true);
    let message = fixture.request(lead).prompt.id;
    fixture.now += SHELL_HOLD_MS - 10;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(status["requests"][0]["turn_ended"], false, "{status}");

    fixture.now += 10;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(status["requests"][0]["turn_ended"], true, "{status}");
    assert_eq!(
        status["requests"][0]["reason"], "background_shells_timeout",
        "{status}"
    );
    // The agent's queue moves on.
    fixture.deliver();
    assert_eq!(
        fixture.typed().last().unwrap(),
        &("review the next change".into(), false)
    );
    assert_eq!(fixture.request(next).phase, RequestPhase::Active);
}

/// The cap counts from the first hold; a shell finishing wakes a turn that may
/// hold again with newer text, and that newer text is what the cap delivers.
#[test]
fn the_shell_cap_counts_from_the_first_hold_and_delivers_the_newest_interim_text() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    fixture.now += 5 * 60 * 1000;
    fixture.started(
        "turn-2",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop(
        "turn-2",
        "Build done; waiting on the server.",
        &["b2"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.now += SHELL_HOLD_MS - 5 * 60 * 1000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Build done; waiting on the server.")
    );
}

/// The cap is the `background_shell_hold_minutes` Bus setting.
#[test]
fn the_shell_cap_is_the_background_shell_hold_minutes_setting() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let settings = fixture.dir.join("settings.json");
    std::fs::write(&settings, br#"{"background_shell_hold_minutes":1}"#).unwrap();
    fixture.worker.settings_path = Some(settings);
    let lead = held_on_a_started_shell(&mut fixture);
    fixture.now += 60 * 1000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
}

/// Regression (live, 2026-10-13): a developer's MASTER message and a worker's
/// blocker notice both went to an orchestrator whose reply was held for shells
/// its turn started. The agent sat Idle, nothing was typed, and both messages
/// were abandoned as `queued_stalled` two minutes later. The agent takes
/// input while it waits. Messages folded into one turn share its reply;
/// messages answered in a later turn keep that turn's own answer.
#[test]
fn messages_to_an_agent_holding_its_reply_for_shells_are_typed_into_that_turn() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let question = fixture.send("is the first pr ready to merge?", false);
    let blocker = fixture.send("Blocked, needs help to continue.", false);
    fixture.deliver();
    fixture.deliver();
    assert_eq!(
        fixture.typed()[1..],
        [
            ("is the first pr ready to merge?".to_owned(), true),
            ("Blocked, needs help to continue.".to_owned(), true)
        ]
    );
    for id in [question, blocker] {
        assert_eq!(fixture.request(id).group, Some(lead));
    }
    fixture
        .worker
        .expire_queued_requests(fixture.now + QUEUED_STALL_MS + 1)
        .unwrap();
    for id in [lead, question, blocker] {
        assert_ne!(fixture.request(id).phase, RequestPhase::Abandoned);
    }

    // Claude runs the typed input as a turn of its own while the build runs,
    // the second message joining it mid-turn...
    fixture.started("turn-2", "is the first pr ready to merge?");
    fixture.consume();
    fixture.started("turn-2", "Blocked, needs help to continue.");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop("turn-2", "Not yet; the build runs.", &["b1"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    // ...and the build finishing wakes the turn that carries the reply.
    fixture.started(
        "turn-3",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop("turn-3", "Build green; pr 1 is ready.", &[]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for id in [question, blocker] {
        assert_eq!(
            fixture.reply(id).as_deref(),
            Some("Not yet; the build runs.")
        );
    }
}

/// A `--queue` message waits for the held reply instead of being typed into
/// it, and the shell hold (bounded by its own cap) is not a queued stall.
#[test]
fn a_queued_message_waits_out_a_shell_hold_without_being_abandoned() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let next = fixture.send("review the next change", true);
    fixture.deliver();
    assert_eq!(fixture.typed().len(), 1);
    fixture
        .worker
        .expire_queued_requests(fixture.now + QUEUED_STALL_MS + 1)
        .unwrap();
    assert_eq!(fixture.request(next).phase, RequestPhase::Queued);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
}

/// E2E: a `--queue` message waited out a shell hold, then was abandoned as
/// `queued_stalled` in the moment between the shell's wake-up turn starting
/// (which ends the hold) and Bus seeing the agent Working. The agent's turns
/// on the request ahead are progress, so the queue has not stalled; it is
/// delivered once that request settles, even when its last turn started and
/// ended between two status polls.
#[test]
fn a_queued_message_survives_the_wake_up_turn_that_ends_a_shell_hold() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(&mut fixture, "build it", "turn-1");
    // The hold and the message behind it began past the stall threshold.
    let now = fixture.now;
    fixture.now -= QUEUED_STALL_MS + 60_000;
    fixture.record(claude_stop("turn-1", "Waiting on the build.", &["b1"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    let next = fixture.send("review the next change", true);
    fixture.now = now;
    fixture.started(
        "turn-2",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.deliver();
    assert_eq!(fixture.request(next).phase, RequestPhase::Queued);

    fixture.record(claude_stop("turn-2", "Build done.", &[]));
    fixture.consume();
    fixture.now = crate::messaging::storage::io::now_ms();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.reply(lead).as_deref(), Some("Build done."));
    fixture.deliver();
    assert_eq!(
        fixture.typed().last().unwrap(),
        &("review the next change".into(), false)
    );
    assert_eq!(fixture.request(next).phase, RequestPhase::Active);
}

/// Holds `fixture`'s request on a recurring session cron, as Claude reports
/// it: Stop lists the cron and retains the reply until the background cap.
fn held_on_a_session_cron(fixture: &mut Fixture) -> RequestId {
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(fixture, "watch the deploy", "turn-1");
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1","last_assistant_message":"Checking every five minutes.","background_tasks":[],"session_crons":[{"cron_id":"cron-1","status":"running"}]}));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    lead
}

/// A queued message waits on a captured cron reply's bounded hold.
#[test]
fn a_queued_message_behind_a_captured_cron_reply_survives_until_its_cap() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_session_cron(&mut fixture);
    let next = fixture.send("review the next change", true);
    fixture.deliver();
    fixture.now += QUEUED_STALL_MS + 1000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.worker.expire_queued_requests(fixture.now).unwrap();
    assert_eq!(fixture.request(next).phase, RequestPhase::Queued);
    fixture.now += SHELL_HOLD_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    fixture.worker.expire_queued_requests(fixture.now).unwrap();
    fixture.deliver();
    assert_eq!(fixture.request(next).phase, RequestPhase::Active);
}

/// A new message answered in its own turn must preserve the held answer of
/// the preceding turn instead of replacing it at the shell cap.
#[test]
fn a_message_answered_in_a_new_turn_keeps_the_preceding_held_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let question = fixture.send("is the first pr ready to merge?", false);
    fixture.deliver();
    fixture.started("turn-2", "is the first pr ready to merge?");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop(
        "turn-2",
        "Not yet; the build runs.",
        &["b1", "b2"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    assert_eq!(fixture.request(question).group, None);
    fixture.now += SHELL_HOLD_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    assert_eq!(
        fixture.reply(question).as_deref(),
        Some("Not yet; the build runs.")
    );
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("background_shells_new_turn")
    );
    assert_eq!(
        fixture.request(question).failure_reason.as_deref(),
        Some("background_shells_timeout")
    );
}

/// A message typed into a held turn whose start Claude never reported still
/// settles with the group when the shell cap releases it.
#[test]
fn an_unreported_message_typed_into_a_held_turn_settles_at_the_shell_cap() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let question = fixture.send("is the first pr ready to merge?", false);
    fixture.deliver();
    fixture.now += SHELL_HOLD_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(fixture.request(question).phase, RequestPhase::Completed);
}

/// disk2 replied while its file search still ran. A held reply is known
/// progress with its own cap, not failed transcript capture.
#[test]
fn a_shell_held_reply_does_not_stall_its_sender_or_block_later_messages() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    fixture.now += crate::messaging::model::DELIVERED_STALL_MS + 60_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    let status = fixture
        .worker
        .control_message_at(fixture.request(lead).prompt.id, fixture.now)
        .unwrap();
    assert_eq!(status["requests"][0]["stage"], "delivered", "{status}");
    assert_eq!(status["requests"][0]["reason"], Value::Null, "{status}");
    assert_eq!(status["requests"][0]["turn_ended"], false);
    let next = fixture.send("go, run the approved cleanup", false);
    fixture.deliver();
    assert_eq!(fixture.request(next).group, Some(lead));
    assert_eq!(
        fixture.typed().last().unwrap(),
        &("go, run the approved cleanup".into(), true)
    );
    fixture.now += SHELL_HOLD_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(fixture.request(next).phase, RequestPhase::Completed);
    let later = fixture.send("next task", true);
    fixture.deliver();
    assert_eq!(fixture.request(later).phase, RequestPhase::Active);
}

/// Shell-to-cron transitions retain the same bounded reply semantics as a
/// request that first held on a cron.
#[test]
fn a_cron_hold_after_an_ended_shell_hold_is_reported_like_a_cron_hold() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    fixture.now += 60_000;
    fixture.started(
        "turn-2",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-2","last_assistant_message":"Build done; checking the deploy every five minutes.","background_tasks":[],"session_crons":[{"cron_id":"cron-1","status":"running"}]}));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.now += crate::messaging::model::DELIVERED_STALL_MS + 60_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    let held = fixture
        .worker
        .control_message_at(fixture.request(lead).prompt.id, fixture.now)
        .unwrap();

    let mut cron = Fixture::new(Provider::ClaudeCode);
    let cron_lead = held_on_a_session_cron(&mut cron);
    cron.now += crate::messaging::model::DELIVERED_STALL_MS + 60_000;
    cron.status(RuntimeStatus::Idle);
    cron.consume();
    let cron_status = cron
        .worker
        .control_message_at(cron.request(cron_lead).prompt.id, cron.now)
        .unwrap();
    assert_eq!(
        held["requests"][0]["stage"], cron_status["requests"][0]["stage"],
        "held: {held}\ncron: {cron_status}"
    );
}

/// Live (MASTER, 2026-10-16 16:44, requests 13199/13201/13213/13215): the
/// orchestrator's reply was captured but held for shells its turn started; a
/// developer message typed into the held turn joined it; the next `Stop` held
/// again. Then a shell finished and Claude woke a turn, which Bus took as the
/// continuation carrying the real reply (clearing the hold), and that turn
/// ended with no `Stop` (Claude folded the next task notification into it).
/// Settlement abandoned the whole group as `reply_not_captured` and the held
/// text was lost; the late `Stop` was rejected with no active request. A reply
/// Bus captured is never lost: the interim is the group's reply.
#[test]
fn a_wake_turn_ending_without_stop_delivers_the_shell_held_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let joined = fixture.send("let me know when I can test the add agent thing", false);
    fixture.deliver();
    assert_eq!(fixture.request(joined).group, Some(lead));
    fixture.started("turn-2", "let me know when I can test the add agent thing");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop(
        "turn-2",
        "#78 was tested with e2e-test; waiting on its review.",
        &["b1", "b2"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert!(fixture.request(joined).awaiting_background);
    // This request's shell b2 finishes: its wake turn ends the hold...
    fixture.started(
        "turn-3",
        "<task-notification>b2 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    assert!(!fixture.request(joined).awaiting_background);
    // ...and the agent is Idle 2 s later with no Stop hook.
    fixture.now += 2_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert!(fixture.request(joined).turn_ended_at_ms.is_some());
    fixture.now += HELD_REPLY_GRACE_MS + 1;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(joined).phase, RequestPhase::Completed);
    assert_eq!(
        fixture.reply(joined).as_deref(),
        Some("#78 was tested with e2e-test; waiting on its review.")
    );
    assert_eq!(
        fixture.request(joined).failure_reason.as_deref(),
        Some("background_shells_interim")
    );
    assert_eq!(fixture.error(), None);
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        None
    );
    // The Stop that comes later is the agent's own activity: no error, and the
    // settled replies stand.
    fixture.record(claude_stop(
        "turn-3",
        "The add-agent fix is ready to test.",
        &[],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.error(), None);
    assert_eq!(
        fixture.reply(joined).as_deref(),
        Some("#78 was tested with e2e-test; waiting on its review.")
    );
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
}

/// Live (MASTER, 2026-10-16 16:40, requests 13175/13188): Bus was stopped
/// while the wake turn ran and resumed Claude; the agent came back Idle with
/// no `Stop` for that turn. The held interim is still the reply.
#[test]
fn a_restart_under_the_wake_turn_delivers_the_shell_held_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    fixture.started(
        "turn-2",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    // Bus goes down and comes back: a fresh worker on the same state, the
    // session resumed, the agent Idle again.
    let dir = fixture.dir.clone();
    let native = fixture.native.clone();
    let parked = Worker::open(
        dir.join("restart-parking"),
        Box::new(Native(native.clone())),
    )
    .unwrap();
    drop(std::mem::replace(&mut fixture.worker, parked));
    fixture.worker = Worker::open(dir, Box::new(Native(native))).unwrap();
    fixture.status(RuntimeStatus::Unavailable);
    fixture
        .record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"resume"}));
    fixture.consume();
    fixture.now += 3_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += HELD_REPLY_GRACE_MS + 1;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("background_shells_interim")
    );
}

/// A turn that ended without a `Stop` and never held a reply still closes as
/// `reply_not_captured`: no reply is invented.
#[test]
fn a_turn_without_a_held_reply_still_closes_as_reply_not_captured() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(&mut fixture, "build it", "turn-1");
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += CALLBACK_GRACE_MS + 1;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
}

/// Live (2026-10-16): the orchestrator's replies were held behind its own
/// `bus send --async` waits on workers. Those are not work the reply waits
/// on, so a Stop listing only Bus waits is the final reply at once.
#[test]
fn a_claude_reply_is_not_held_by_the_agents_own_bus_waits() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(&mut fixture, "dispatch the review", "turn-1");
    fixture.record(json!({
        "hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
        "last_assistant_message":"Review dispatched; I'll report when it lands.",
        "background_tasks":[
            {"id":"w1","type":"shell","status":"running","description":"Wait","command":"bus send --async --room qa --as orch --to rev --text 'review #1'"},
            {"id":"w2","type":"shell","status":"running","description":"Wait","command":"/opt/bus/bin/bus --dev wait 12"}
        ],
        "session_crons":[]
    }));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Review dispatched; I'll report when it lands.")
    );
}

/// E2E (scratch Bus, 2026-10-16): the fake Claude's `Stop` reached Bus 6 s
/// after its screen went idle, past the 5 s settle window, so the held interim
/// had already become the reply and the real one was posted as a stray report.
/// Claude's `Stop` can trail the idle screen (its hooks run first), so a turn
/// holding a reply waits `HELD_REPLY_SETTLE_MS` for a late `Stop`, which then
/// settles the turn as usual: as the reply, or held again for its shells.
#[test]
fn a_late_stop_within_the_held_reply_window_is_still_the_turn_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let joined = fixture.send("is it ready?", false);
    fixture.deliver();
    fixture.started("turn-1", "is it ready?");
    fixture.consume();
    fixture.started(
        "turn-2",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert!(fixture.request(lead).turn_ended_at_ms.is_some());
    // Past the plain settle window the turn is still open for its Stop...
    fixture.now += CALLBACK_GRACE_MS + 1_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    assert!(fixture.worker.state.settling(fixture.request(joined)));
    // ...and that Stop, with no shell left running, is the reply.
    fixture.record(claude_stop("turn-2", "Ready: the build passed.", &[]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    for id in [lead, joined] {
        assert_eq!(fixture.request(id).phase, RequestPhase::Completed);
        assert_eq!(
            fixture.reply(id).as_deref(),
            Some("Ready: the build passed.")
        );
        assert_eq!(fixture.request(id).failure_reason, None);
    }
}

/// A wake turn ended without a `Stop` (say Bus restarted under it), and the
/// turn's second shell finishes inside the held-reply window: Claude wakes a
/// new turn for it, whose `Stop` carries the real reply. That captured reply,
/// not the older interim, is what the held messages get.
#[test]
fn a_second_wake_turn_inside_the_held_reply_window_carries_the_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(&mut fixture, "build it", "turn-1");
    fixture.record(claude_stop(
        "turn-1",
        "Waiting on the build.",
        &["b1", "b2"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert!(fixture.request(lead).awaiting_background);
    fixture.started(
        "turn-2",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.now += 2_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert!(fixture.request(lead).turn_ended_at_ms.is_some());
    fixture.now += 10_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.started(
        "turn-3",
        "<task-notification>b2 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.now += 5_000;
    fixture.record(claude_stop("turn-3", "Both builds passed.", &[]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += HELD_REPLY_GRACE_MS + 1;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(fixture.reply(lead).as_deref(), Some("Both builds passed."));
}

/// Live MASTER regression: five messages were answered by five distinct
/// Claude turns while the same background shell kept every Stop held.
#[test]
fn five_shell_held_message_turns_keep_their_own_replies() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let first = held_on_a_started_shell(&mut fixture);
    let mut messages = vec![(first, "Waiting on the build.".to_owned())];
    for n in 2..=5 {
        let prompt = format!("question {n}");
        let answer = format!("answer {n}");
        let request = fixture.send(&prompt, false);
        fixture.deliver();
        fixture.started(&format!("turn-{n}"), &prompt);
        fixture.consume();
        fixture.status(RuntimeStatus::Working);
        fixture.record(claude_stop(&format!("turn-{n}"), &answer, &["b1"]));
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        messages.push((request, answer));
    }
    fixture.now += SHELL_HOLD_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for (index, (id, answer)) in messages.iter().enumerate() {
        assert_eq!(fixture.request(*id).phase, RequestPhase::Completed);
        assert_eq!(
            fixture.reply(*id).as_deref(),
            Some(answer.as_str()),
            "message {} lost its own turn reply",
            index + 1
        );
        assert_eq!(fixture.request(*id).group, None);
        let status = fixture
            .worker
            .control_message_at(fixture.request(*id).prompt.id, fixture.now)
            .unwrap();
        assert_eq!(
            status["requests"][0]["reply"]["provider_turn_id"],
            format!("turn-{}", index + 1)
        );
    }
}

/// A message answered in its own turn while an earlier request's shell still
/// runs keeps that answer when the shell later wakes a turn of its own: the
/// wake turn reports on the earlier request's work, not this message.
#[test]
fn a_message_answered_in_its_own_turn_keeps_its_reply_when_an_earlier_shell_wakes() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let question = fixture.send("did the docs pr merge?", false);
    fixture.deliver();
    fixture.started("turn-2", "did the docs pr merge?");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop("turn-2", "Yes, the docs pr merged.", &["b1"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.started(
        "turn-3",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop("turn-3", "Build green; pr 1 is ready.", &[]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    assert_eq!(
        fixture.reply(question).as_deref(),
        Some("Yes, the docs pr merged.")
    );
}

/// Two messages typed into a shell-held turn, where Claude starts a turn for
/// the first and folds the second into it without a prompt of its own: the
/// new turn answers both, and the held request keeps its own interim.
#[test]
fn messages_folded_into_a_new_turn_after_a_held_reply_share_that_turns_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let first = fixture.send("check the docs pr", false);
    let folded = fixture.send("and the changelog", false);
    fixture.deliver();
    fixture.deliver();
    fixture.started("turn-2", "check the docs pr");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop(
        "turn-2",
        "Docs pr and changelog are fine.",
        &[],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now =
        crate::messaging::storage::io::now_ms() + crate::messaging::model::STEERING_SETTLE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    for id in [first, folded] {
        assert_eq!(fixture.request(id).phase, RequestPhase::Completed);
        assert_eq!(
            fixture.reply(id).as_deref(),
            Some("Docs pr and changelog are fine.")
        );
    }
}

/// An older request's shell can wake while the new request still owns a
/// different running shell. Its report must not replace the new held answer.
#[test]
fn an_inherited_shell_wake_cannot_replace_the_current_requests_held_answer() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let question = fixture.send("review the docs", false);
    fixture.deliver();
    fixture.started("turn-2", "review the docs");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop(
        "turn-2",
        "Docs reviewed; tests still run.",
        &["b1", "b2"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.started(
        "turn-3",
        "<task-notification><task-id>b1</task-id>completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop("turn-3", "Earlier build completed.", &["b2"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += SHELL_HOLD_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    assert_eq!(
        fixture.reply(question).as_deref(),
        Some("Docs reviewed; tests still run.")
    );
}

/// An actual Bus message can contain the same text as a task notification.
/// Its matching submit callback still owns the new turn and its reply.
#[test]
fn a_bus_message_matching_an_inherited_task_notification_keeps_its_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = held_on_a_started_shell(&mut fixture);
    let prompt = "<task-notification><task-id>b1</task-id>completed</task-notification>";
    let question = fixture.send(prompt, false);
    fixture.deliver();
    fixture.started("turn-2", prompt);
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop(
        "turn-2",
        "That is a task notification.",
        &["b1"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.now += HELD_REPLY_GRACE_MS + 1;
    fixture.consume();
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    assert_eq!(
        fixture.reply(question).as_deref(),
        Some("That is a task notification.")
    );
}

/// A rollout timestamp `offset_ms` after the fixture's clock.
fn rollout_at(fixture: &Fixture, offset_ms: i64) -> String {
    let ms = i128::from(fixture.now) + i128::from(offset_ms);
    time::OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

/// Codex 0.163.0 rollout lines for one turn (shapes measured 2026-10-20).
fn codex_turn(at: &str, turn: &str, prompt: &str, answer: Option<&str>) -> String {
    let mut lines = vec![
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_started","turn_id":turn}}),
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"item_completed","turn_id":turn,
            "item":{"type":"UserMessage","content":[{"type":"text","text":prompt}]}}}),
    ];
    if let Some(answer) = answer {
        lines.push(
            json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_complete",
            "turn_id":turn,"last_agent_message":answer}}),
        );
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn append(path: &std::path::Path, lines: &str) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
}

/// Runs the agent's turn to Idle with no `Stop` and waits out the settle window.
fn end_without_stop(fixture: &mut Fixture) {
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
}

fn assert_reply_not_captured(fixture: &Fixture, id: RequestId) {
    assert_eq!(fixture.request(id).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture.request(id).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
}

/// Live (2026-10-20, Codex 0.163.0): a worker removed its own worktree, its
/// cwd, during the turn. Codex then ran no `Stop` hook, though the turn
/// finished and its rollout recorded the answer, and Bus abandoned the
/// request as `reply_not_captured`.
#[test]
fn a_codex_turn_ending_without_stop_delivers_the_answer_its_rollout_recorded() {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
    let lead = fixture.send("merge the PR", false);
    fixture.deliver();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"merge the PR","transcript_path":rollout}),
    );
    fixture.consume();
    assert!(fixture.request(lead).trusted_start_bound);
    let at = rollout_at(&fixture, 1);
    append(
        &rollout,
        &codex_turn(&at, "turn-1", "merge the PR", Some("Merged #1627.")),
    );
    end_without_stop(&mut fixture);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(fixture.reply(lead).as_deref(), Some("Merged #1627."));
    assert!(fixture.error().is_none());
    // A late Stop for the same turn is not a second reply.
    fixture.stopped("turn-1", "Merged #1627.");
    fixture.consume();
    let replies = &fixture
        .worker
        .state
        .room(fixture.room)
        .unwrap()
        .latest_replies;
    assert_eq!(replies.len(), 1);
}

/// Live (2026-10-20): the follow-up to the turn above was never bound, since
/// Codex ran no hook at all once its cwd was gone. Its rollout turn is the one
/// that ran the typed prompt after Bus sent it.
#[test]
fn an_unbound_codex_turn_delivers_the_answer_of_the_rollout_turn_that_ran_its_prompt() {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
    fixture.record(
        json!({"hook_event_name":"SessionStart","session_id":"session",
        "source":"startup","transcript_path":rollout}),
    );
    fixture.consume();
    // An identical turn older than the 60 s clock margin is not a candidate.
    let earlier = rollout_at(&fixture, -120_000);
    append(
        &rollout,
        &codex_turn(&earlier, "turn-0", "close the worktree", Some("old")),
    );
    let lead = fixture.send("close the worktree", false);
    fixture.deliver();
    assert!(!fixture.request(lead).trusted_start_bound);
    let typed = fixture.typed().last().unwrap().0.clone();
    let at = rollout_at(&fixture, 1);
    append(
        &rollout,
        &codex_turn(&at, "turn-2", &typed, Some("Copied 374 files.")),
    );
    end_without_stop(&mut fixture);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(fixture.reply(lead).as_deref(), Some("Copied 374 files."));
}

/// An unbound request whose prompt no rollout turn ran since it was sent
/// cannot be given an answer: a turn the agent ran on its own, or an older
/// turn with the same text, is not its turn.
#[test]
fn an_unbound_codex_turn_without_its_prompt_in_the_rollout_stays_reply_not_captured() {
    for own_turn in [true, false] {
        let mut fixture = Fixture::new(Provider::Codex);
        let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
        fixture.record(
            json!({"hook_event_name":"SessionStart","session_id":"session",
            "source":"startup","transcript_path":rollout}),
        );
        fixture.consume();
        let lead = fixture.send("close the worktree", false);
        fixture.deliver();
        let typed = fixture.typed().last().unwrap().0.clone();
        let (at, prompt) = if own_turn {
            (rollout_at(&fixture, 1), "something else".to_owned())
        } else {
            (rollout_at(&fixture, -1), typed)
        };
        append(
            &rollout,
            &codex_turn(&at, "turn-2", &prompt, Some("not this")),
        );
        end_without_stop(&mut fixture);
        assert_reply_not_captured(&fixture, lead);
    }
}

/// A bound turn whose rollout records no answer for it (the turn's
/// `task_complete` is missing, names another turn, or carries no message)
/// still closes as `reply_not_captured`: no reply is invented.
#[test]
fn a_codex_turn_whose_rollout_has_no_answer_for_it_stays_reply_not_captured() {
    for recorded in [
        codex_turn("2026-10-20T05:06:51.361Z", "turn-1", "merge the PR", None),
        codex_turn(
            "2026-10-20T05:06:51.361Z",
            "turn-other",
            "merge the PR",
            Some("other"),
        ),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1",
            "last_agent_message":null}})
        .to_string()
            + "\n",
    ] {
        let mut fixture = Fixture::new(Provider::Codex);
        let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
        append(&rollout, &recorded);
        let lead = fixture.send("merge the PR", false);
        fixture.deliver();
        fixture.record(
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
            "turn_id":"turn-1","prompt":"merge the PR","transcript_path":rollout}),
        );
        fixture.consume();
        end_without_stop(&mut fixture);
        assert_reply_not_captured(&fixture, lead);
    }
}

/// Verifier regression (2026-10-20, temp/bug-reply-not-captured section 6):
/// the earlier of two turns that ran the same prompt since it was sent sat
/// outside the rollout tail Bus read, so the later turn's answer was
/// delivered to the earlier request. Every turn since the send must be seen;
/// two matching turns are ambiguous and deliver nothing.
#[test]
fn truncated_tail_must_not_assign_a_later_identical_turn_to_an_earlier_request() {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
    fixture.record(
        json!({"hook_event_name":"SessionStart","session_id":"session",
        "source":"startup","transcript_path":rollout}),
    );
    fixture.consume();
    let id = fixture.send("status?", false);
    fixture.deliver();
    let typed = fixture.typed().last().unwrap().0.clone();
    append(
        &rollout,
        &codex_turn(&rollout_at(&fixture, 1), "first", &typed, None),
    );
    // Real rollout reasoning and tool output often exceed 256 KiB.
    let reasoning = json!({"timestamp":rollout_at(&fixture, 2),"type":"response_item",
        "payload":{"type":"reasoning","encrypted_content":"x".repeat(300 * 1024)}});
    let complete = json!({"timestamp":rollout_at(&fixture, 60_000),"type":"event_msg",
        "payload":{"type":"task_complete","turn_id":"first","last_agent_message":"FIRST"}});
    append(&rollout, &format!("{reasoning}\n{complete}\n"));
    let at = rollout_at(&fixture, 60_001);
    append(&rollout, &codex_turn(&at, "later", &typed, Some("LATER")));
    fixture.now += 60_002;
    end_without_stop(&mut fixture);
    assert!(fixture.reply(id).is_none(), "{:?}", fixture.reply(id));
    assert_reply_not_captured(&fixture, id);
}

#[test]
fn reply_capture_recovered_log_is_not_written_for_an_unsaved_settlement() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "prompt", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture
        .worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        assert!(fixture.worker.settle_requests().is_err());
        fixture.worker.settle_requests().unwrap();
    });
    let log = capture.text();
    let rows = log
        .lines()
        .filter(|line| {
            line.contains("bus.message.recovered")
                && line.contains("reply_not_captured")
                && line.contains(&format!("request_id={} ", lead.0))
        })
        .count();
    assert_eq!(rows, 1, "{log}");
}
