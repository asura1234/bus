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
            fixture.typed_content(),
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
        assert_eq!(fixture.request(request).delivery_stage(), "replied");
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
    assert_eq!(
        fixture.typed_content(),
        [("separate turn".to_owned(), false)]
    );
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
        .observe_dialog(
            fixture.agent,
            true,
            fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .observed_at_ms,
        )
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
        let status = fixture.worker.control_message(message).unwrap();
        assert_eq!(status["requests"][0]["stage"], "replied", "{status}");
        assert_eq!(
            status["requests"][0]["reply"]["text"], "all three answered",
            "{status}"
        );
    }
}

fn claude_steer_batch_with_image(image_message: &str) -> (Fixture, RequestId, RequestId, String) {
    let mut fixture = Fixture::in_room(Provider::ClaudeDs, true);
    fixture.status(RuntimeStatus::Launching);
    let mut state = fixture.worker.state.clone();
    let room = state.create_room("work").unwrap();
    let sender = state
        .create_agent(
            room,
            "reporter",
            Provider::ClaudeDs,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(fixture.agent, room).unwrap();
    let image = fixture.dir.join("screenshot.png");
    let first = state
        .submit_message_from(
            room,
            Draft {
                text: image_message.into(),
                files: vec![image.clone()],
                recipient_ids: [fixture.agent].into(),
            },
            Author::Agent(sender),
            fixture.now,
        )
        .unwrap()[0];
    state
        .record_send_mode(&[first], crate::messaging::model::SendMode::Steer)
        .unwrap();
    let second = state
        .submit_message_from(
            room,
            Draft {
                text: "The worker has been idle for five minutes.".into(),
                files: Vec::new(),
                recipient_ids: [fixture.agent].into(),
            },
            Author::Bus,
            fixture.now,
        )
        .unwrap()[0];
    fixture.worker.save(state).unwrap();
    fixture.deliver();
    assert!(fixture.typed().is_empty());
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    let typed = fixture.typed();
    assert_eq!(typed.len(), 1);
    assert!(
        !typed[0].1,
        "the steer envelope was delivered as a standalone batch"
    );
    assert_eq!(fixture.request(second).group, Some(first));
    assert!(!fixture.request(second).steered);
    let path_and_separator = format!("\n\"{}\"\n\n", image.display());
    assert!(typed[0].0.contains(&path_and_separator));
    assert!(typed[0].0.contains("画布 / 开始"));
    // Match the recorded hook shape: one leading chip, a complete paste frame,
    // and a newline in place of the space before the literal slash.
    let body = typed[0]
        .0
        .replace(&path_and_separator, "\n")
        .replace("画布 / 开始", "画布\n/ 开始");
    let payload = format!(
        "[Image #435]\n\n<pasted_content id=\"3c97\">\n{body}\n</pasted_content id=\"3c97\">\n"
    );
    (fixture, first, second, payload)
}

#[test]
fn claude_steer_batch_with_image_binds_and_settles_both_requests() {
    let (mut fixture, first, second, payload) =
        claude_steer_batch_with_image("设备前台显示「双击画布 / 开始创作」。");
    fixture.started("batch", &payload);
    fixture.consume();
    assert!(
        fixture.request(first).trusted_start_bound,
        "Started must bind the frozen batch after the image and paste transformations"
    );
    fixture.stopped("batch", "Both messages received.");
    fixture.consume();
    for request in [first, second] {
        assert_eq!(fixture.request(request).phase, RequestPhase::Completed);
        assert_eq!(
            fixture.reply(request).as_deref(),
            Some("Both messages received.")
        );
        assert_eq!(fixture.request(request).reply_key(), first);
        assert!(fixture.request(request).failure_reason.is_none());
    }
}

#[test]
fn claude_steer_batch_with_image_rejects_unrelated_turn_and_keeps_warning() {
    let (mut fixture, first, second, payload) =
        claude_steer_batch_with_image("设备前台显示「双击画布 / 开始创作」。");
    let foreign = payload.replace(
        "The worker has been idle",
        "A different worker has been idle",
    );
    assert_ne!(foreign, payload);
    fixture.started("foreign", &foreign);
    fixture.stopped("foreign", "An answer to different input.");
    fixture.consume();
    assert!(!fixture.request(first).trusted_start_bound);
    assert!(fixture.request(first).foreign_turn_settled_at_ms.is_some());

    // A subsequent autonomous turn proves the agent has moved on.
    fixture.started("next", "A new autonomous turn.");
    fixture.consume();
    for request in [first, second] {
        assert_eq!(fixture.request(request).phase, RequestPhase::Abandoned);
        assert_eq!(
            fixture.request(request).failure_reason.as_deref(),
            Some("submission_unconfirmed")
        );
        assert!(fixture.reply(request).is_none());
    }
    assert!(fixture.error().is_some());
}

fn claude_batch_with_wrapped_inline_file(
    evidence_reference: &str,
) -> (Fixture, RequestId, RequestId, String) {
    let caption = format!(
        "设备前台显示「双击画布 / 开始创作」。`build` / channel `local`。证据 {evidence_reference}。继续。"
    );
    let (fixture, lead, member, payload) = claude_steer_batch_with_image(&caption);
    let payload = payload
        .replace("`build` / channel", "`build`\n/ channel")
        .replace("证据 /work", "证据\n/work");
    (fixture, lead, member, payload)
}

#[test]
fn claude_consumed_turn_with_wrapped_inline_file_settles_each_batch_member() {
    for evidence in ["/work/images/screenshot.png", "/work/report.md "] {
        let (mut fixture, lead, member, payload) = claude_batch_with_wrapped_inline_file(evidence);
        assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
        assert!(!fixture.request(lead).trusted_start_bound);
        fixture.started("batch", &payload);
        fixture.consume();
        assert!(
            fixture.request(lead).trusted_start_bound,
            "Started must bind the full payload with the wrapped path {evidence:?}"
        );
        fixture.stopped("batch", "I received the screenshot.");
        fixture.consume();
        for request in [lead, member] {
            assert_eq!(fixture.request(request).delivery_stage(), "replied");
            assert_eq!(
                fixture.reply(request).as_deref(),
                Some("I received the screenshot.")
            );
            assert_eq!(fixture.request(request).reply_key(), lead);
            assert!(fixture.request(request).failure_reason.is_none());
        }
        assert!(fixture.error().is_none());
    }
}

#[test]
fn claude_consumed_turn_with_changed_inline_png_stays_unrelated() {
    let (mut fixture, lead, member, payload) =
        claude_batch_with_wrapped_inline_file("/work/images/screenshot.png");
    let foreign = payload.replace("/work/images/screenshot.png", "/work/images/different.png");
    fixture.started("foreign", &foreign);
    fixture.stopped("foreign", "An answer to different input.");
    fixture.consume();
    assert!(!fixture.request(lead).trusted_start_bound);

    fixture.started("next", "A new autonomous turn.");
    fixture.consume();
    for request in [lead, member] {
        assert_eq!(fixture.request(request).phase, RequestPhase::Abandoned);
        assert_eq!(
            fixture.request(request).failure_reason.as_deref(),
            Some("submission_unconfirmed")
        );
        assert!(fixture.reply(request).is_none());
    }
    assert!(fixture.error().is_some());
}

#[test]
fn queued_cross_room_messages_keep_distinct_replies_in_their_source_rooms() {
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
    assert_eq!(test_prompt_content(&typed), "developer follow-up");
    fixture.started("master-turn", &typed);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Queued);
    assert_eq!(fixture.request(lead).group, None);
    let unread: Vec<_> = [work, master]
        .into_iter()
        .map(|id| fixture.worker.state.room(id).unwrap().unread_count)
        .collect();
    fixture.stopped("master-turn", "follow-up answered");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    let typed = fixture.typed()[1].0.clone();
    // Cross-room delivery keeps the original worker's authenticated header,
    // whose delivered prefix still names its home room.
    assert_eq!(
        test_prompt_content(&typed),
        "builder:\na real dialog needs help"
    );
    assert!(
        typed.contains("(room "),
        "the prefix must keep the room: {typed:?}"
    );
    fixture.started("worker-turn", &typed);
    fixture.stopped("worker-turn", "dialog answered");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    for ((room, request, answer), unread) in [
        (work, lead, "dialog answered"),
        (master, member, "follow-up answered"),
    ]
    .into_iter()
    .zip(unread)
    {
        assert_eq!(
            fixture.reply(request).as_deref(),
            Some(answer),
            "{request:?}"
        );
        assert_eq!(fixture.request(request).delivery_stage(), "replied");
        let room_state = fixture.worker.state.room(room).unwrap();
        assert!(
            !room_state.latest_replies.contains_key(&orchestrator),
            "{:?}",
            room_state.latest_replies
        );
        assert_eq!(
            room_state.unread_count, unread,
            "a turn final is not a source reply"
        );
    }
    let reports: Vec<_> = fixture.worker.state.notices(master).collect();
    assert!(
        reports.is_empty(),
        "an accepted turn final is not posted: {reports:?}"
    );
}
