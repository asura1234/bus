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
