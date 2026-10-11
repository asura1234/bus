use super::*;

#[test]
fn claude_messages_reach_an_unbound_busy_turn_without_waiting_for_idle() {
    for status in [RuntimeStatus::Working, RuntimeStatus::Idle] {
        let mut fixture = Fixture::new(Provider::ClaudeCode);
        fixture.started("own-turn", "a task notification");
        fixture.consume();
        fixture.status(status);
        let started = std::time::Instant::now();
        let request = fixture.send("developer correction", false);
        fixture.deliver();
        assert_eq!(
            fixture.typed(),
            [("developer correction".to_owned(), true)],
            "{status:?}: do not wait for the own-turn grace or an idle poll"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        fixture.started("own-turn", "developer correction");
        fixture.stopped("own-turn", "correction applied");
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        assert_eq!(
            fixture.reply(request).as_deref(),
            Some("correction applied")
        );
    }
}

#[test]
fn queue_only_claude_message_waits_for_an_unbound_turn_to_end() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.started("own-turn", "a task notification");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    let request = fixture.send("separate turn", true);
    fixture.deliver();
    assert!(fixture.typed().is_empty());
    assert_eq!(fixture.request(request).phase, RequestPhase::Queued);
    fixture.stopped("own-turn", "done");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    assert_eq!(fixture.typed(), [("separate turn".to_owned(), false)]);
}

#[test]
fn an_own_turn_does_not_bypass_blocked_unavailable_or_dialog_readiness() {
    for status in [
        RuntimeStatus::Blocked,
        RuntimeStatus::Unavailable,
        RuntimeStatus::Launching,
    ] {
        let mut fixture = Fixture::new(Provider::ClaudeCode);
        fixture.started("own-turn", "a task notification");
        fixture.consume();
        fixture.status(status);
        fixture.send("developer correction", false);
        fixture.deliver();
        assert!(fixture.typed().is_empty(), "{status:?}");
    }
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.started("own-turn", "a task notification");
    fixture.consume();
    fixture
        .worker
        .state
        .observe_dialog(fixture.agent, true)
        .unwrap();
    fixture.send("developer correction", false);
    fixture.deliver();
    assert!(fixture.typed().is_empty());
}

#[test]
fn claude_coalesced_paste_with_adjacent_input_completes_every_joined_message() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.status(RuntimeStatus::Launching);
    let first = fixture.send("status on bus work", false);
    let second = fixture.send("what is still outstanding?", false);
    let third = fixture.send("is slow delivery fixed?", false);
    fixture.deliver();
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    let typed = fixture.typed()[0].0.clone();
    let payload =
        format!("\n\n<pasted_content id=\"f85f\">\n{typed}\n</pasted_content id=\"f85f\">\n\na");
    fixture.started("batch", &payload);
    fixture.stopped("batch", "all three answered");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    for id in [first, second, third] {
        let message = fixture.request(id).prompt.id;
        let status = fixture.worker.dev_message(message).unwrap();
        assert_eq!(status["requests"][0]["stage"], "replied", "{status}");
        assert_eq!(status["requests"][0]["reply"]["text"], "all three answered");
    }
}

#[test]
fn coalesced_cross_room_reply_is_published_in_every_members_room() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let work = fixture.room;
    let mut state = fixture.worker.state.clone();
    let master = state.master_room().unwrap().id;
    let orchestrator = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, work).unwrap();
    let identity = state.agent(fixture.agent).unwrap().runtime_identity.clone();
    state
        .set_agent_runtime_identity(orchestrator, identity)
        .unwrap();
    state.confirm_hook_setup(orchestrator).unwrap();
    state
        .observe_status(orchestrator, RuntimeStatus::Launching, fixture.now)
        .unwrap();
    let lead = state
        .submit_message_from(
            work,
            Draft {
                text: "a real dialog needs help".into(),
                files: Vec::new(),
                recipient_ids: [orchestrator].into(),
            },
            Author::Agent(fixture.agent),
            fixture.now,
        )
        .unwrap()[0];
    fixture.worker.save(state).unwrap();
    // Use the orchestrator's spool identity, just as MASTER uses a work-room notice.
    callbacks::initialize(
        &fixture.dir.join("callbacks/launch"),
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(orchestrator.0),
            provider: fixture.provider,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    fixture.agent = orchestrator;
    fixture.room = master;
    let member = fixture.send("developer follow-up", false);
    fixture.deliver();
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    let typed = fixture.typed()[0].0.clone();
    fixture.started("batch", &typed);
    let unread: Vec<_> = [work, master]
        .into_iter()
        .map(|id| fixture.worker.state.room(id).unwrap().unread_count)
        .collect();
    fixture.stopped("batch", "dialog and follow-up answered");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    for ((room, request), unread) in [(work, lead), (master, member)].into_iter().zip(unread) {
        assert_eq!(
            fixture.reply(request).as_deref(),
            Some("dialog and follow-up answered")
        );
        let room = fixture.worker.state.room(room).unwrap();
        assert_eq!(
            room.latest_replies
                .get(&orchestrator)
                .map(|reply| reply.text.as_str()),
            Some("dialog and follow-up answered")
        );
        assert_eq!(
            room.unread_count,
            unread + 1,
            "one shared reply in each room"
        );
    }
}
