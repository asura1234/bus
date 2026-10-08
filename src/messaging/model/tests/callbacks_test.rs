use super::*;

#[test]
fn confirmed_provider_ids_cannot_bypass_trusted_prompt_start_binding() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "same prompt");
    state
        .begin_submission(request, "launch-codex", 10)
        .expect("begin");
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .expect("confirmed");

    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "premature-final",
            11,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "same prompt",
            "must not publish",
        )),
        CallbackDisposition::Rejected(CallbackRejection::UnboundFinal)
    );
    assert!(state.room(room).expect("room").latest_replies.is_empty());

    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "trusted-start".into(),
            sequence: 12,
            occurred_at_ms: 12,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("same prompt".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "bound-final",
            13,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "same prompt",
            "publish this",
        )),
        CallbackDisposition::AcceptedPendingSettlement
    );
}

#[test]
fn callback_prompt_matching_normalizes_terminal_line_endings_and_releases_queue() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(
        &mut state,
        room,
        agent,
        "first line\r  second line\r\nthird line",
    );
    let queued = submit_text(&mut state, room, agent, "queued next");
    state
        .begin_submission(request, "launch-codex", 10)
        .expect("begin");
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .expect("confirmed");

    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "normalized-start".into(),
            sequence: 11,
            occurred_at_ms: 11,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("first line\n  second line\nthird line".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "normalized-final",
            12,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "first line\n  second line\nthird line",
            "finished",
        )),
        CallbackDisposition::AcceptedPendingSettlement
    );

    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .expect("settled");

    assert_eq!(
        state.request(request).expect("request").phase,
        RequestPhase::Completed
    );
    assert_eq!(state.next_queued_request(agent), Some(queued));
    assert_eq!(
        state.room(room).expect("room").latest_replies[&agent].text,
        "finished"
    );
}

#[test]
fn callback_prompt_matching_rejects_prefilled_composer_text() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "hello?");
    state
        .begin_submission(request, "launch-codex", 20)
        .expect("begin");
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .expect("confirmed");

    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "prefilled-start".into(),
            sequence: 21,
            occurred_at_ms: 21,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("draft already present\nhello?".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::Rejected(CallbackRejection::WrongPrompt)
    );
    assert!(!state.request(request).expect("request").trusted_start_bound);
}

#[test]
fn claude_long_paste_with_attached_file_binds_its_trusted_start() {
    let (mut state, room, _, claude) = state_with_room_and_agents();
    let text = "Please read the attached brief and reply for EACH of F1, F2, F3 with \
                    your recommended option and a 2-4 sentence rationale (file:line). "
        .repeat(8);
    let file = "/private/tmp/scratchpad/pr1131-review/r1-flags.md";
    state.set_draft_text(room, &text).expect("draft text");
    state
        .attach_file(room, PathBuf::from(file))
        .expect("attach");
    state
        .set_draft_recipients(room, [claude])
        .expect("recipients");
    let request = state.submit_draft(room, 10).expect("submit")[0];
    state
        .begin_submission(request, "launch-claude", 15)
        .expect("begin");
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: None,
                provider_turn_id: None,
            },
        )
        .expect("confirmed");

    // Shape captured from Claude Code 2.1.284 for a Bus paste of text plus
    // one quoted attachment path: the whole paste is framed, path included.
    let hook = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "claude-session",
        "prompt_id": "claude-prompt",
        "prompt": format!(
            "\n\n<pasted_content id=\"dff1\">\n{text}\n\"{file}\"\n</pasted_content id=\"dff1\">\n"
        ),
    });
    let crate::bus::callbacks::Parsed::Started {
        session,
        turn,
        prompt,
    } = crate::bus::callbacks::parse(Provider::ClaudeCode, &hook).expect("parse")
    else {
        panic!("UserPromptSubmit must parse as a start");
    };

    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "claude-framed-start".into(),
            sequence: 16,
            occurred_at_ms: 16,
            agent_id: claude,
            launch_id: "launch-claude".into(),
            provider_session_id: Some(session),
            provider_turn_id: Some(turn.clone()),
            provider_prompt_id: Some(turn),
            prompt_payload: Some(prompt),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    assert!(state.request(request).expect("request").trusted_start_bound);
}

#[test]
fn claude_attached_image_binds_its_trusted_start() {
    // Shapes captured from Claude Code 2.1.291: a pasted line that is one
    // quoted image path becomes an `[Image #N]` attachment (N counts the
    // session's images), reported first, and blank lines are dropped; a
    // long remainder keeps its paste framing.
    let short = "Reply with just: pong\n\n\nsecond para  \n\nthird";
    let long = "for orchestrator agents in MASTER I just need \nso it should look like\n\n\
                    bus orchestrator Idle x\n"
        .repeat(4);
    let cases = [
        (
            short,
            "[Image #1]Reply with just: pong\nsecond para  \nthird".to_owned(),
        ),
        (
            long.as_str(),
            format!(
                "[Image #17]\n\n<pasted_content id=\"8f5d\">\n{}\n</pasted_content id=\"8f5d\">\n",
                long.trim_end().replace("\n\n", "\n")
            ),
        ),
        ("", "[Image #3]".to_owned()),
    ];
    for (n, (text, hook_prompt)) in cases.into_iter().enumerate() {
        let (mut state, room, _, claude) = state_with_room_and_agents();
        if !text.is_empty() {
            state.set_draft_text(room, text).expect("draft text");
        }
        state
            .attach_file(room, PathBuf::from("/tmp/bus/paste-e8d63c50.png"))
            .expect("attach");
        state
            .set_draft_recipients(room, [claude])
            .expect("recipients");
        let request = state.submit_draft(room, 10).expect("submit")[0];
        state
            .begin_submission(request, "launch-claude", 15)
            .expect("begin");
        let hook = serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "claude-session",
            "prompt_id": "claude-prompt",
            "prompt": hook_prompt,
        });
        let crate::bus::callbacks::Parsed::Started {
            session,
            turn,
            prompt,
        } = crate::bus::callbacks::parse(Provider::ClaudeCode, &hook).expect("parse")
        else {
            panic!("UserPromptSubmit must parse as a start");
        };
        let start = |id: &str, prompt: String| ProviderCallback {
            callback_id: id.into(),
            sequence: 16,
            occurred_at_ms: 16,
            agent_id: claude,
            launch_id: "launch-claude".into(),
            provider_session_id: Some(session.clone()),
            provider_turn_id: Some(format!("{turn}-{id}")),
            provider_prompt_id: Some(format!("{turn}-{id}")),
            prompt_payload: Some(prompt),
            kind: CallbackEventKind::PromptStarted,
        };
        // Different text, or more images than the request attached, is another turn.
        for (wrong, payload) in [
            ("text", format!("{prompt} extra")),
            ("images", format!("[Image #9]{prompt}")),
        ] {
            assert_ne!(
                state.accept_callback(start(wrong, payload)),
                CallbackDisposition::AcceptedBinding,
                "case {n}: {wrong}"
            );
        }
        assert_eq!(
            state.accept_callback(start("image-start", prompt)),
            CallbackDisposition::AcceptedBinding,
            "case {n}"
        );
        assert!(state.request(request).expect("request").trusted_start_bound);
    }
}

#[test]
fn final_is_joined_with_settled_status_and_routes_by_stable_ids_not_visible_room_or_name() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let other = state.create_room("other").expect("other");
    state.select_room(other).expect("select other");
    let request = submit_text(&mut state, room, agent, "build it");
    start_request(&mut state, request, "launch-codex", 8);
    state
        .rename_room(room, "renamed mid-flight")
        .expect("rename room");
    state
        .rename_agent(agent, "renamed agent")
        .expect("rename agent");

    let disposition = state.accept_callback(ProviderCallback {
        callback_id: "callback-1".into(),
        sequence: 10,
        occurred_at_ms: 25,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: None,
        prompt_payload: Some("build it".into()),
        kind: CallbackEventKind::Final {
            text: "finished".into(),
        },
    });
    assert_eq!(disposition, CallbackDisposition::AcceptedPendingSettlement);
    assert!(state.room(room).expect("room").latest_replies.is_empty());

    state
        .observe_status(agent, RuntimeStatus::Idle, 30)
        .expect("idle");

    let reply = &state.room(room).expect("room").latest_replies[&agent];
    assert_eq!(reply.text, "finished");
    assert_eq!(reply.request_id, request);
    assert_eq!(state.room(room).expect("room").unread_count, 1);
    assert_eq!(state.room(other).expect("other").unread_count, 0);
    assert_eq!(
        state.request(request).expect("request").phase,
        RequestPhase::Completed
    );
    assert_eq!(state.next_queued_request(agent), None);
    state.select_room(room).expect("read room");
    assert_eq!(state.room(room).expect("room").unread_count, 0);
}

#[test]
fn previous_reply_remains_until_correlated_final_and_visible_room_does_not_accrue_unread() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.select_room(room).expect("select room");
    let old = submit_text(&mut state, room, agent, "old prompt");
    start_request(&mut state, old, "launch-codex", 1);
    state.accept_callback(ProviderCallback::final_event(
        "old-final",
        3,
        agent,
        "launch-codex",
        "provider-session",
        "turn-1",
        "old prompt",
        "old reply",
    ));
    state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .expect("settled");

    let new_request = submit_text(&mut state, room, agent, "new prompt");
    state
        .begin_submission(new_request, "launch-codex", 3)
        .expect("begin");
    state
        .record_submission(
            new_request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-2".into()),
            },
        )
        .expect("active");
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "new-trusted-start".into(),
            sequence: 4,
            occurred_at_ms: 11,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-2".into()),
            provider_prompt_id: None,
            prompt_payload: Some("new prompt".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    state
        .observe_status(agent, RuntimeStatus::Blocked, 11)
        .expect("blocked");

    assert_eq!(
        state.room(room).expect("room").latest_replies[&agent].text,
        "old reply"
    );
    assert_eq!(state.room(room).expect("room").unread_count, 0);

    state.accept_callback(ProviderCallback::final_event(
        "new-final",
        5,
        agent,
        "launch-codex",
        "provider-session",
        "turn-2",
        "new prompt",
        "new reply",
    ));
    state
        .observe_status(agent, RuntimeStatus::Idle, 12)
        .expect("settled");
    assert_eq!(
        state.room(room).expect("room").latest_replies[&agent].text,
        "new reply"
    );
    assert_eq!(state.room(room).expect("room").unread_count, 0);
}

#[test]
fn duplicate_wrong_session_and_stale_pre_submit_finals_are_rejected() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "same prompt");
    start_request(&mut state, request, "launch-codex", 100);

    let stale = ProviderCallback::final_event(
        "stale",
        100,
        agent,
        "launch-codex",
        "provider-session",
        "turn-1",
        "same prompt",
        "stale",
    );
    assert_eq!(
        state.accept_callback(stale),
        CallbackDisposition::Rejected(CallbackRejection::BeforeSubmissionBoundary)
    );
    let wrong = ProviderCallback::final_event(
        "wrong",
        102,
        agent,
        "launch-codex",
        "other-session",
        "turn-1",
        "same prompt",
        "wrong",
    );
    assert_eq!(
        state.accept_callback(wrong),
        CallbackDisposition::Rejected(CallbackRejection::WrongSession)
    );
    let valid = ProviderCallback::final_event(
        "valid",
        103,
        agent,
        "launch-codex",
        "provider-session",
        "turn-1",
        "same prompt",
        "valid",
    );
    assert_eq!(
        state.accept_callback(valid.clone()),
        CallbackDisposition::AcceptedPendingSettlement
    );
    assert_eq!(
        state.accept_callback(valid),
        CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback)
    );
}

#[test]
fn uncertain_submission_is_not_retried_but_authoritative_callback_can_finish_it() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "deploy");
    state
        .begin_submission(request, "launch-codex", 40)
        .expect("begin");
    state
        .record_submission(
            request,
            SubmissionOutcome::Uncertain {
                message: "connection lost after write".into(),
            },
        )
        .expect("uncertain");

    assert_eq!(state.next_queued_request(agent), None);
    assert!(state.request(request).expect("request").uncertain_outcome);
    assert_eq!(
        state
            .agent(agent)
            .expect("agent")
            .actionable_error
            .as_deref(),
        Some("connection lost after write")
    );

    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "start-without-payload".into(),
            sequence: 41,
            occurred_at_ms: 41,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("wrong-session".into()),
            provider_turn_id: Some("wrong-turn".into()),
            provider_prompt_id: None,
            prompt_payload: None,
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::Rejected(CallbackRejection::WrongPrompt)
    );

    let unbound_final = ProviderCallback::final_event(
        "unbound-final",
        42,
        agent,
        "launch-codex",
        "new-session",
        "new-turn",
        "deploy",
        "wrong",
    );
    assert_eq!(
        state.accept_callback(unbound_final),
        CallbackDisposition::Rejected(CallbackRejection::UnboundFinal)
    );
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "trusted-start".into(),
            sequence: 43,
            occurred_at_ms: 45,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("new-session".into()),
            provider_turn_id: Some("new-turn".into()),
            provider_prompt_id: Some("prompt-9".into()),
            prompt_payload: Some("deploy".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "late-authoritative",
            44,
            agent,
            "launch-codex",
            "new-session",
            "new-turn",
            "deploy",
            "done",
        )),
        CallbackDisposition::AcceptedPendingSettlement
    );
    state
        .observe_status(agent, RuntimeStatus::Idle, 50)
        .expect("idle");
    assert_eq!(
        state.request(request).expect("request").phase,
        RequestPhase::Completed
    );
}

#[test]
fn error_callback_is_visible_and_never_becomes_a_reply() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "prompt");
    start_request(&mut state, request, "launch-codex", 1);

    let result = state.accept_callback(ProviderCallback {
        callback_id: "error-event".into(),
        sequence: 3,
        occurred_at_ms: 15,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: None,
        prompt_payload: Some("prompt".into()),
        kind: CallbackEventKind::Error {
            message: "provider session aborted".into(),
        },
    });

    assert_eq!(result, CallbackDisposition::AcceptedError);
    assert!(state.room(room).expect("room").latest_replies.is_empty());
    assert_eq!(
        state
            .agent(agent)
            .expect("agent")
            .actionable_error
            .as_deref(),
        Some("provider session aborted")
    );
    assert_eq!(
        state.request(request).expect("request").phase,
        RequestPhase::Active
    );
}

#[test]
fn idle_observed_before_submission_is_not_enough_to_settle_a_later_final() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Idle, 1)
        .expect("pre-submit idle");
    let request = submit_text(&mut state, room, agent, "prompt");
    start_request(&mut state, request, "launch-codex", 10);

    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "final",
            12,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "prompt",
            "done",
        )),
        CallbackDisposition::AcceptedPendingSettlement
    );
    assert_eq!(
        state.request(request).expect("request").phase,
        RequestPhase::Active
    );

    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .expect("post-final settled idle");
    assert_eq!(
        state.request(request).expect("request").phase,
        RequestPhase::Completed
    );
}

#[test]
fn background_progress_and_same_session_continuation_wait_for_the_later_final() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "review the plan");
    start_request(&mut state, request, "launch-codex", 10);

    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "background-pending".into(),
            sequence: 12,
            occurred_at_ms: 12,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: None,
            kind: CallbackEventKind::BackgroundPending,
        }),
        CallbackDisposition::AcceptedProgress
    );
    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .expect("provider paused for background work");
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Active);

    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "continuation-start".into(),
            sequence: 14,
            occurred_at_ms: 14,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-2".into()),
            provider_prompt_id: None,
            prompt_payload: Some("<task-notification>reviewer finished</task-notification>".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedContinuation
    );
    assert_eq!(
        state.request(request).unwrap().provider_turn_id.as_deref(),
        Some("turn-2")
    );
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "continuation-final",
            15,
            agent,
            "launch-codex",
            "provider-session",
            "turn-2",
            "review the plan",
            "Ready",
        )),
        CallbackDisposition::AcceptedCompleted
    );
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        state.room(room).unwrap().latest_replies[&agent].text,
        "Ready"
    );
}
