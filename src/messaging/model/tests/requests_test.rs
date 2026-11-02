use super::*;

#[test]
fn pure_wait_reason_preserves_precedence_and_codex_first_session_exception() {
    let (state, id, request) = idle_with_queued(1_000, "go");
    let mut agent = state.agent(id).unwrap().clone();
    agent.status = RuntimeStatus::Idle;
    agent.hook_setup_confirmed = true;
    agent.runtime_identity.launch_id = Some("launch".into());
    agent.runtime_identity.terminal_id = Some("terminal".into());
    agent.runtime_identity.pane_id = Some("pane".into());
    agent.runtime_identity.session_id = None;
    agent.current_request = None;
    agent.provider = Provider::Codex;
    assert_eq!(agent.wait_reason(), None);
    agent.dialog = true;
    assert_eq!(agent.shown_status(), RuntimeStatus::Blocked);
    assert_eq!(agent.wait_reason(), None);
    agent.provider = Provider::ClaudeCode;
    assert_eq!(agent.wait_reason(), Some("session_hook_missing"));
    agent.runtime_identity.pane_id = None;
    assert_eq!(agent.wait_reason(), Some("terminal_identity_missing"));
    agent.current_request = Some(request);
    assert_eq!(agent.wait_reason(), Some("prior_request_active"));
    for (status, reason) in [
        (RuntimeStatus::Unavailable, "agent_unavailable"),
        (RuntimeStatus::Launching, "agent_launching"),
        (RuntimeStatus::Working, "agent_working"),
        (RuntimeStatus::Blocked, "agent_blocked"),
    ] {
        agent.status = status;
        assert_eq!(agent.wait_reason(), Some(reason));
    }
    agent.hook_setup_confirmed = false;
    assert_eq!(agent.wait_reason(), Some("hook_setup_unconfirmed"));
    agent.session_binding_invalidated = true;
    assert_eq!(agent.wait_reason(), Some("session_invalidated"));
    agent.deletion_pending = true;
    assert_eq!(agent.wait_reason(), Some("deletion_pending"));
}

#[test]
fn submission_preserves_first_recipient_selection_order() {
    let (mut state, room, codex, claude) = state_with_room_and_agents();
    state
        .set_draft_recipients(room, [claude, codex, claude])
        .expect("ordered recipients");
    state.set_draft_text(room, "review this").expect("draft");

    let requests = state.submit_draft(room, 99).expect("submit");
    let request_agents = requests
        .iter()
        .map(|id| state.request(*id).expect("request").agent_id)
        .collect::<Vec<_>>();
    let prompt = &state.request(requests[0]).expect("request").prompt;

    assert_eq!(request_agents, [claude, codex]);
    assert_eq!(
        prompt.recipient_ids.iter().copied().collect::<Vec<_>>(),
        [claude, codex]
    );
    let restored: BusState =
        serde_json::from_value(serde_json::to_value(&state).expect("serialize"))
            .expect("deserialize");
    assert_eq!(
        restored
            .request(requests[0])
            .expect("restored request")
            .prompt
            .recipient_ids
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [claude, codex]
    );
}

#[test]
fn file_only_submission_builds_stable_per_agent_requests_and_clears_only_draft() {
    let (mut state, room, codex, claude) = state_with_room_and_agents();
    let first_path = PathBuf::from("/one/report.md");
    let second_path = PathBuf::from("/two/report.md");
    state.attach_file(room, first_path.clone()).expect("attach");
    state
        .attach_file(room, second_path.clone())
        .expect("attach");
    state
        .attach_file(room, first_path.clone())
        .expect("deduplicate");
    state.remove_file(room, &second_path).expect("remove");
    state
        .attach_file(room, second_path.clone())
        .expect("reattach");
    state
        .set_draft_recipients(room, [codex, claude])
        .expect("recipients");

    let requests = state.submit_draft(room, 99).expect("file-only submit");

    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0], requests[1]);
    let prompt = state.request(requests[0]).expect("request").prompt.clone();
    assert_eq!(prompt.text, "");
    assert_eq!(prompt.files, vec![first_path, second_path]);
    assert_eq!(
        prompt.rendered_payload(),
        "\"/one/report.md\" \"/two/report.md\""
    );
    assert!(state.room(room).expect("room").draft.files.is_empty());
    assert_eq!(
        state
            .room(room)
            .expect("room")
            .latest_prompt
            .as_ref()
            .expect("latest")
            .files,
        prompt.files
    );
}

#[test]
fn turn_ended_needs_work_after_submission_then_idle_and_ignores_reply_capture() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Working, 1)
        .unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 2).unwrap();
    let first = submit_text(&mut state, room, agent, "first");
    let second = submit_text(&mut state, room, agent, "second");
    let ended = |state: &BusState, id| state.turn_ended(state.request(id).unwrap());
    assert!(!ended(&state, first), "queued");

    // Typed, but no turn start reported: only status polls tell work here.
    submit_request(&mut state, first, "launch-codex", 5);
    // Work seen before the submission does not count, nor idle without work.
    state.observe_status(agent, RuntimeStatus::Idle, 3).unwrap();
    assert!(!ended(&state, first));
    state
        .observe_status(agent, RuntimeStatus::Working, 4)
        .unwrap();
    assert!(!ended(&state, first));
    // A blocked agent is not idle.
    state
        .observe_status(agent, RuntimeStatus::Blocked, 5)
        .unwrap();
    assert!(!ended(&state, first));
    state.observe_status(agent, RuntimeStatus::Idle, 6).unwrap();
    // No reply was captured, yet the turn ended.
    assert_eq!(state.request(first).unwrap().phase, RequestPhase::Active);
    assert!(ended(&state, first));
    assert!(!ended(&state, second), "the next message is still queued");

    state.requests.get_mut(&first).unwrap().phase = RequestPhase::Completed;
    assert!(ended(&state, first), "a captured reply counts too");
    state.requests.get_mut(&first).unwrap().phase = RequestPhase::Abandoned;
    assert!(!ended(&state, first));
}

#[test]
fn kimi_intermediate_idle_does_not_end_its_started_turn() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run shell and edit a file");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    assert!(!state.turn_ended(state.request(request).unwrap()));
    assert!(state
        .settle_ended_request(agent, 100_000, |_| None)
        .is_none());
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Active);
    state
        .observe_status(agent, RuntimeStatus::Working, 100_001)
        .unwrap();
    let payload = state.request(request).unwrap().prompt.rendered_payload();
    let final_event = ProviderCallback::final_event(
        "kimi-final",
        100_002,
        agent,
        "launch-codex",
        "provider-session",
        "turn-1",
        &payload,
        "done",
    );
    assert!(matches!(
        state.accept_callback(final_event),
        CallbackDisposition::AcceptedCompleted | CallbackDisposition::AcceptedPendingSettlement
    ));
}

#[test]
fn kimi_terminal_failure_can_still_settle_without_a_final_reply() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    let ended = RecordedTurnEnd::Error("Kimi turn failed".into());
    assert_eq!(
        state.settle_ended_request(agent, 100_000, |_| Some(ended)),
        Some(request)
    );
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(state
        .request(request)
        .unwrap()
        .failure_reason
        .as_deref()
        .unwrap()
        .contains("Kimi"));
}

#[test]
fn kimi_successful_silent_turn_completes_with_an_empty_reply() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(agent, 100_000, |_| Some(RecordedTurnEnd::NoReply)),
        Some(request)
    );
    let settled = state.request(request).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert_eq!(settled.reply().unwrap().text, "");
    assert!(settled.failure_reason.is_none());
}

#[test]
fn kimi_completed_wire_recovers_one_final_reply_after_intermediate_idle() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    assert!(state
        .settle_ended_request(agent, 100_000, |_| None)
        .is_none());
    let prompt = state.request(request).unwrap().prompt.rendered_payload();
    let record = RecordedTurnEnd::Final {
        turn_id: "turn-1".into(),
        text: "finished".into(),
        prompts: vec![prompt],
    };
    assert_eq!(
        state.settle_ended_request(agent, 100_001, |_| Some(record)),
        Some(request)
    );
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert!(state
        .settle_ended_request(agent, 100_002, |_| None)
        .is_none());
}

#[test]
fn kimi_process_exit_settles_confirmed_submission_before_start_hook() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    state.begin_submission(request, "launch-codex", 10).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    assert!(!state.request(request).unwrap().trusted_start_bound);
    state
        .record_exit_snapshot(
            agent,
            ExitSnapshot {
                launch_id: Some("launch-codex".into()),
                terminal_id: "terminal".into(),
                path: None,
                error: Some("process exited".into()),
                tail: String::new(),
                at_ms: 100,
                terminal_closed: None,
            },
        )
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 100)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(agent, 10_000, |_| None),
        Some(request),
        "a confirmed submission must settle when its provider exits"
    );
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
}

#[test]
fn uncertain_submission_cannot_end_from_unrelated_status_activity() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "first");
    state.begin_submission(request, "launch-codex", 5).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Uncertain {
                message: "timeout".into(),
            },
        )
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Blocked, 10)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 11)
        .unwrap();
    assert!(!state.turn_ended(state.request(request).unwrap()));
    assert!(state
        .settle_ended_request(agent, 100_000, |_| None)
        .is_none());
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
}

#[test]
fn a_queued_message_refused_by_the_input_box_recovers_with_that_reason() {
    let (mut state, agent, request) = idle_with_queued(1_000, "go");
    state.begin_submission(request, "launch-codex", 5).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::DefinitelyRejected {
                message: "Claude input box is not empty; prompt was not sent".into(),
            },
        )
        .unwrap();
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Queued);
    assert_eq!(
        state.agent(agent).unwrap().delivery_rejection.as_deref(),
        Some("input_box_not_empty")
    );
    assert_eq!(
        recovery_at(&state, request, 1_000 + QUEUED_STALL_MS - 1),
        None
    );
    assert_eq!(
        recovery_at(&state, request, 1_000 + QUEUED_STALL_MS).as_deref(),
        Some("input_box_not_empty")
    );
    // A confirmed submission clears the refusal.
    state.begin_submission(request, "launch-codex", 6).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: None,
                provider_turn_id: None,
            },
        )
        .unwrap();
    assert_eq!(state.agent(agent).unwrap().delivery_rejection, None);
}

#[test]
fn a_queued_message_without_a_refusal_recovers_with_the_wait_reason() {
    let (state, agent, request) = idle_with_queued(1_000, "go");
    let expected = state
        .agent(agent)
        .unwrap()
        .wait_reason()
        .unwrap_or("not_submitted");
    assert_eq!(
        recovery_at(&state, request, 1_000 + QUEUED_STALL_MS).as_deref(),
        Some(expected)
    );
}

#[test]
fn an_unconfirmed_submission_gets_a_recovery_reason_after_its_idle_grace() {
    // The clock starts at the poll before submission (1_000), not at later idle polls.
    let (mut state, agent, submitting) = idle_with_queued(1_000, "a");
    state
        .begin_submission(submitting, "launch-codex", 5)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 2_000)
        .unwrap();
    assert_eq!(
        recovery_at(&state, submitting, 1_000 + DELIVERED_STALL_MS - 1),
        None
    );
    assert_eq!(
        recovery_at(&state, submitting, 1_000 + DELIVERED_STALL_MS).as_deref(),
        Some("submission_unconfirmed")
    );
}

#[test]
fn a_long_block_is_reported_after_its_grace() {
    let (mut state, agent, request) = idle_with_queued(1_000, "go");
    start_request(&mut state, request, "launch-codex", 5);
    state
        .observe_status(agent, RuntimeStatus::Working, 2_000)
        .unwrap();
    let hour = 60 * 60 * 1000;
    assert!(!state.blocked_unanswered(state.request(request).unwrap(), 2_000 + hour));

    state
        .observe_status(agent, RuntimeStatus::Blocked, 3_000)
        .unwrap();
    let blocked = |now| state.blocked_unanswered(state.request(request).unwrap(), now);
    assert!(!blocked(3_000 + BLOCKED_STALL_MS - 1));
    assert!(blocked(3_000 + BLOCKED_STALL_MS));
}

#[test]
fn a_turn_between_two_idle_polls_still_ends_through_its_provider_start() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let request = submit_text(&mut state, room, agent, "quick task");
    submit_request(&mut state, request, "launch-codex", 5);
    // The whole turn runs between two polls: every poll sees Idle, and
    // the reply is never captured. Only the provider's turn start says
    // the agent took the message.
    state.observe_status(agent, RuntimeStatus::Idle, 2).unwrap();
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "fast-start".into(),
            sequence: 12,
            occurred_at_ms: 12,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("quick task".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    let ended = |state: &BusState| state.turn_ended(state.request(request).unwrap());
    // Idle seen before the start event does not end the turn.
    assert!(!ended(&state));
    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .unwrap();
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Active);
    assert!(ended(&state));
}

#[test]
fn queued_requests_remain_fifo_while_active_request_is_blocked_twice() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let first = submit_text(&mut state, room, agent, "first");
    let second = submit_text(&mut state, room, agent, "second");
    let third = submit_text(&mut state, room, agent, "third");

    assert_eq!(state.next_queued_request(agent), Some(first));
    start_request(&mut state, first, "launch-codex", 5);
    state
        .observe_status(agent, RuntimeStatus::Blocked, 20)
        .expect("blocked");
    state
        .observe_status(agent, RuntimeStatus::Working, 21)
        .expect("working");
    state
        .observe_status(agent, RuntimeStatus::Blocked, 22)
        .expect("blocked again");
    state
        .observe_status(agent, RuntimeStatus::Working, 23)
        .expect("working again");

    assert_eq!(
        state.request(first).expect("first").phase,
        RequestPhase::Active
    );
    assert_eq!(state.next_queued_request(agent), None);
    assert_eq!(state.queued_requests(agent), &[second, third]);
}

#[test]
fn recovery_abandons_only_an_idle_current_request_and_releases_its_agent() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "wedged request");
    let queued = submit_text(&mut state, room, agent, "next request");
    start_request(&mut state, request, "launch-codex", 10);

    assert_eq!(
        state.recover_idle_request(request, 20),
        Err(ModelError::AgentNotIdle)
    );
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    state.recover_idle_request(request, 22).unwrap();

    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(state.request(request).unwrap().completed_at_ms, Some(22));
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert_eq!(state.next_queued_request(agent), Some(queued));
    assert!(state.room(room).unwrap().latest_replies.is_empty());
}

#[test]
fn every_provider_empty_final_is_durable_and_has_no_room_publication() {
    for provider in [
        Provider::ClaudeCode,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        for text in ["", " \t\n"] {
            let (mut state, room, agent, requester) = state_with_room_and_agents();
            state.agents.get_mut(&agent).unwrap().provider = provider;
            let request = state
                .submit_message_from(
                    room,
                    Draft {
                        text: "finish silently".into(),
                        files: vec![],
                        recipient_ids: [agent].into(),
                    },
                    Author::Agent(requester),
                    10,
                )
                .unwrap()[0];
            start_request(&mut state, request, "launch-codex", 10);
            let unread = state.room(room).unwrap().unread_count;
            let prompt = state.request(request).unwrap().prompt.rendered_payload();
            assert_eq!(
                state.accept_callback(ProviderCallback::final_event(
                    "empty-final",
                    12,
                    agent,
                    "launch-codex",
                    "provider-session",
                    "turn-1",
                    &prompt,
                    text
                )),
                CallbackDisposition::AcceptedCompleted,
                "{provider:?}"
            );
            let mut restored: BusState =
                serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
            let reply = restored.request(request).unwrap();
            assert_eq!(reply.phase, RequestPhase::Completed);
            assert_eq!(reply.reply().unwrap().text, text, "{provider:?}");
            assert!(reply.failure_reason.is_none());
            assert_eq!(restored.agent(agent).unwrap().current_request, None);
            assert!(restored.room(room).unwrap().latest_replies.is_empty());
            assert_eq!(restored.room(room).unwrap().unread_count, unread);
            restored
                .route_worker_replies(100_000, &std::collections::BTreeSet::new())
                .unwrap();
            assert_eq!(
                restored.requests().count(),
                1,
                "{provider:?}: empty final generated a message"
            );
        }
    }
}

#[test]
fn orchestrator_independent_admission_preserves_developer_worker_and_queue_neighbours() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Kimi] {
        for orchestrator in [false, true] {
            for (agent_authored, explicit_queue, generated) in [
                (true, false, false),
                (false, false, false),
                (true, true, false),
                (false, true, false),
                (true, false, true),
            ] {
                let (mut state, work, sender, _) = state_with_room_and_agents();
                let target_room = if orchestrator {
                    state.ensure_master_room()
                } else {
                    work
                };
                let target = state
                    .create_agent(target_room, "target", provider, "/repo".into(), None)
                    .unwrap();
                if orchestrator {
                    state.bind_orchestrator(target, work).unwrap();
                }
                let identity = state.agent(sender).unwrap().runtime_identity.clone();
                state.set_agent_runtime_identity(target, identity).unwrap();
                let author = if agent_authored {
                    Author::Agent(sender)
                } else {
                    Author::Developer
                };
                let send = |state: &mut BusState| {
                    let draft = Draft {
                        text: "same text".into(),
                        files: vec![],
                        recipient_ids: [target].into(),
                    };
                    if generated {
                        state
                            .submit_bus_message_with(
                                work,
                                draft,
                                author.clone(),
                                10,
                                explicit_queue,
                            )
                            .unwrap()[0]
                    } else {
                        state
                            .submit_message_with(work, draft, author.clone(), 10, explicit_queue)
                            .unwrap()[0]
                    }
                };
                let lead = send(&mut state);
                let next = send(&mut state);
                let independent = explicit_queue || (orchestrator && agent_authored && !generated);
                let mut restored: BusState =
                    serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
                assert_eq!(restored.coalesce_queue(lead).is_none(), independent,
                    "{provider:?} orchestrator={orchestrator} agent={agent_authored} queue={explicit_queue} generated={generated}");
                // Isolate steering from the coalescing assertion on the clone.
                start_request(&mut state, lead, "launch-codex", 20);
                state
                    .observe_status(target, RuntimeStatus::Working, 22)
                    .unwrap();
                assert_eq!(
                    state.next_steering(target),
                    (!independent).then_some((next, lead))
                );
            }
        }
    }
}

#[test]
fn orchestrator_independent_boundary_stops_coalescing_from_both_sides() {
    for agent_first in [false, true] {
        let (mut state, work, sender, _) = state_with_room_and_agents();
        let target = master_agent(&mut state, "orch");
        state.bind_orchestrator(target, work).unwrap();
        let mut requests = vec![];
        for agent_authored in [agent_first, !agent_first] {
            requests.push(
                state
                    .submit_message_with(
                        work,
                        Draft {
                            text: "question".into(),
                            files: vec![],
                            recipient_ids: [target].into(),
                        },
                        if agent_authored {
                            Author::Agent(sender)
                        } else {
                            Author::Developer
                        },
                        10,
                        false,
                    )
                    .unwrap()[0],
            );
        }
        assert!(state.coalesce_queue(requests[0]).is_none());
        assert_eq!(state.queued_requests(target), requests);
        assert!(state.request(requests[1]).unwrap().group.is_none());
    }
}

#[test]
fn orchestrator_independent_policy_is_per_recipient_in_a_shared_message() {
    for orchestrator_first in [false, true] {
        let (mut state, room, worker, sender) = state_with_room_and_agents();
        let orch = master_agent(&mut state, "orch");
        state.bind_orchestrator(orch, room).unwrap();
        let mut identity = state.agent(worker).unwrap().runtime_identity.clone();
        identity.launch_id = Some("launch-orch".into());
        state.set_agent_runtime_identity(orch, identity).unwrap();
        let worker_lead = submit_text(&mut state, room, worker, "worker task");
        let orch_lead = submit_text(&mut state, room, orch, "orchestrator task");
        start_request(&mut state, worker_lead, "launch-codex", 20);
        start_request(&mut state, orch_lead, "launch-orch", 20);
        for agent in [worker, orch] {
            state
                .observe_status(agent, RuntimeStatus::Working, 22)
                .unwrap();
        }
        let recipients = if orchestrator_first {
            [orch, worker]
        } else {
            [worker, orch]
        };
        let requests = state
            .submit_message_from(
                room,
                Draft {
                    text: "shared report".into(),
                    files: vec![],
                    recipient_ids: recipients.into(),
                },
                Author::Agent(sender),
                30,
            )
            .unwrap();
        let request_for = |agent| {
            *requests
                .iter()
                .find(|id| state.request(**id).unwrap().agent_id == agent)
                .unwrap()
        };
        let (worker_request, orch_request) = (request_for(worker), request_for(orch));
        assert_eq!(
            state.request(worker_request).unwrap().prompt.id,
            state.request(orch_request).unwrap().prompt.id
        );
        assert_eq!(
            state.next_steering(worker),
            Some((worker_request, worker_lead))
        );
        assert_eq!(
            state.next_steering(orch),
            None,
            "orchestrator must queue regardless of recipient order"
        );
        state.begin_steering(worker_request, worker_lead).unwrap();
        assert!(state.begin_steering(orch_request, orch_lead).is_err());
        assert_eq!(
            state.request(orch_request).unwrap().phase,
            RequestPhase::Queued
        );
        assert_eq!(state.request(orch_request).unwrap().group, None);
    }
}

#[test]
fn orchestrator_independent_policy_covers_legacy_queued_records_without_the_flag() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Kimi] {
        let (mut state, room, sender, _) = state_with_room_and_agents();
        let master = state.ensure_master_room();
        let target = state
            .create_agent(master, "orch", provider, "/repo".into(), None)
            .unwrap();
        state.bind_orchestrator(target, room).unwrap();
        let identity = state.agent(sender).unwrap().runtime_identity.clone();
        state.set_agent_runtime_identity(target, identity).unwrap();
        let lead = submit_text(&mut state, room, target, "running task");
        start_request(&mut state, lead, "launch-codex", 20);
        state
            .observe_status(target, RuntimeStatus::Working, 22)
            .unwrap();
        let mut queued = vec![];
        for _ in 0..2 {
            queued.push(
                state
                    .submit_message_from(
                        room,
                        Draft {
                            text: "legacy question".into(),
                            files: vec![],
                            recipient_ids: [target].into(),
                        },
                        Author::Agent(sender),
                        30,
                    )
                    .unwrap()[0],
            );
        }
        let mut saved = serde_json::to_value(state).unwrap();
        for id in &queued {
            saved["requests"]
                .as_object_mut()
                .unwrap()
                .get_mut(&id.0.to_string())
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove("queue_only");
        }
        let mut restored: BusState = serde_json::from_value(saved).unwrap();
        assert!(
            !restored.request(queued[0]).unwrap().queue_only,
            "old records default false"
        );
        assert_eq!(
            restored.next_steering(target),
            None,
            "legacy request must not join the running turn"
        );
        assert!(restored.coalesce_queue(queued[0]).is_none());
        assert_eq!(restored.queued_requests(target), queued.as_slice());
    }
}

#[test]
fn delivery_timestamps_round_trip_and_legacy_requests_default_to_empty() {
    let (mut state, _, request) = idle_with_queued(1_000, "timed");
    let accepted = state.request(request).unwrap().prompt.submitted_at_ms;
    assert_eq!(
        state.request(request).unwrap().latency.accepted_at_ms,
        Some(accepted)
    );
    state.record_queued(request, 1_005).unwrap();
    state.record_queued(request, 900).unwrap();
    assert_eq!(
        state.request(request).unwrap().latency.queued_at_ms,
        Some(1_005),
        "a retry or backward wall clock cannot replace the first timestamp"
    );
    let mut document = serde_json::to_value(&state).unwrap();
    document["requests"][request.0.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("latency");
    let restored: BusState = serde_json::from_value(document).unwrap();
    assert!(restored.request(request).unwrap().latency.is_empty());
    let round_trip: BusState =
        serde_json::from_value(serde_json::to_value(&restored).unwrap()).unwrap();
    assert_eq!(round_trip, restored);
}
