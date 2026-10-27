use super::*;

// The callback spool gets a bounded grace period after the first settled poll.
const CALLBACK_GRACE_MS: u64 = crate::messaging::model::TURN_SETTLE_MS;

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
            fixture.typed_content().last().unwrap(),
            &("review the next change".into(), false)
        );
    }
}

#[test]
fn a_turn_end_without_visible_reply_text_unblocks_the_next_request() {
    for provider in [Provider::Cursor, Provider::ClaudeCode] {
        let expected = if provider == Provider::ClaudeCode {
            RequestPhase::Completed
        } else {
            RequestPhase::Abandoned
        };
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
            // A completed Claude hook with no assistant text is an empty final.
            fixture.record(
                json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1"}),
            );
        }
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        assert_eq!(
            fixture.request(lead).phase,
            if provider == Provider::ClaudeCode {
                expected
            } else {
                RequestPhase::Active
            }
        );
        fixture.now += CALLBACK_GRACE_MS;
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        assert_eq!(fixture.request(lead).phase, expected, "{provider:?}");
        if provider == Provider::ClaudeCode {
            assert_eq!(fixture.request(lead).reply().unwrap().text, "");
            assert!(fixture.request(lead).failure_reason.is_none());
        }
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
            fixture.typed_content().last().unwrap(),
            &("review the next change".into(), false)
        );
        let persisted = fixture.worker.store.load().unwrap().unwrap();
        assert_eq!(persisted.request(lead).unwrap().phase, expected);
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
        fixture.typed_content().last().unwrap(),
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

fn answered_with_a_running_shell(fixture: &mut Fixture) -> RequestId {
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(fixture, "run the build", "turn-1");
    fixture.record(claude_stop("turn-1", "Waiting on the build.", &["b1"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    lead
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

/// Live MASTER regression: five messages were answered by five distinct
/// Claude turns while the same background shell kept every Stop held.
#[test]
fn five_shell_held_message_turns_keep_their_own_replies() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let first = answered_with_a_running_shell(&mut fixture);
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
    let lead = answered_with_a_running_shell(&mut fixture);
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
    let lead = answered_with_a_running_shell(&mut fixture);
    let first = fixture.send("check the docs pr", false);
    fixture.deliver();
    fixture.started("turn-2", "check the docs pr");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    let folded = fixture.send("and the changelog", false);
    fixture.deliver();
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
    let lead = answered_with_a_running_shell(&mut fixture);
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
    let lead = answered_with_a_running_shell(&mut fixture);
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
