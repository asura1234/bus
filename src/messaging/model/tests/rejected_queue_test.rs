use super::*;

fn fixture(provider: Provider) -> (BusState, RoomId, AgentId, RequestId) {
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(room, "recipient", provider, "/scratch".into(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 1_000)
        .unwrap();
    let head = send(&mut state, room, agent, "rejected head", 1_000);
    reject(&mut state, head, 1_020);
    (state, room, agent, head)
}

fn send(state: &mut BusState, room: RoomId, agent: AgentId, text: &str, at: u64) -> RequestId {
    state
        .submit_message_from(
            room,
            Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            at,
        )
        .unwrap()[0]
}

fn reject(state: &mut BusState, id: RequestId, at: u64) {
    let agent = state.request(id).unwrap().agent_id;
    state
        .observe_status(agent, RuntimeStatus::Idle, at)
        .unwrap();
    let launch = state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .launch_id
        .clone()
        .unwrap();
    state.begin_submission(id, &launch, 0).unwrap();
    state
        .record_submission(
            id,
            SubmissionOutcome::DefinitelyRejected {
                message: crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE.into(),
            },
        )
        .unwrap();
}

#[test]
fn rejected_head_expires_despite_unrelated_turns_for_every_provider() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        for status in [
            RuntimeStatus::Idle,
            RuntimeStatus::Working,
            RuntimeStatus::Blocked,
        ] {
            let (mut state, _, agent, head) = fixture(provider);
            for n in 1..=5 {
                let at = 1_020 + n * 20_000;
                state
                    .observe_status(agent, RuntimeStatus::Working, at - 10)
                    .unwrap();
                reject(&mut state, head, at);
            }
            let deadline = 1_020 + QUEUED_STALL_MS;
            state.observe_status(agent, status, deadline - 1).unwrap();
            assert!(state
                .expire_stalled_queued_requests(deadline - 1)
                .unwrap()
                .is_empty());
            assert_eq!(
                state.expire_stalled_queued_requests(deadline).unwrap(),
                vec![head],
                "{provider:?}/{status:?}"
            );
            assert_eq!(
                state.request(head).unwrap().failure_reason.as_deref(),
                Some("prompt_not_shown")
            );
            assert_eq!(state.agent(agent).unwrap().status, status);
        }
    }
}

#[test]
fn rejected_head_releases_later_messages_from_master_and_several_rooms() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let (mut state, room, agent, head) = fixture(provider);
        let master = state.ensure_master_room();
        let other = state.create_room("other").unwrap();
        let later: Vec<_> = [master, room, other]
            .into_iter()
            .map(|r| send(&mut state, r, agent, "later", 90_000))
            .collect();
        state
            .observe_status(agent, RuntimeStatus::Working, 120_999)
            .unwrap();
        assert_eq!(
            state.expire_stalled_queued_requests(121_020).unwrap(),
            vec![head]
        );
        assert_eq!(state.queued_requests(agent), later);
        state
            .observe_status(agent, RuntimeStatus::Idle, 121_021)
            .unwrap();
        assert_eq!(state.next_queued_request(agent), Some(later[0]));
    }
}

#[test]
fn rejected_head_deadline_survives_bus_restart_and_agent_resume_or_relaunch() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        for relaunch in [false, true] {
            let (state, room, agent, head) = fixture(provider);
            let mut state: BusState =
                serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
            state
                .observe_status(agent, RuntimeStatus::Launching, 100_000)
                .unwrap();
            if relaunch {
                state
                    .set_agent_runtime_identity(
                        agent,
                        AgentRuntimeIdentity {
                            launch_id: Some("new-launch".into()),
                            terminal_id: Some("new-terminal".into()),
                            pane_id: Some("new-pane".into()),
                            session_id: Some("new-session".into()),
                        },
                    )
                    .unwrap();
            }
            state
                .observe_status(agent, RuntimeStatus::Idle, 120_999)
                .unwrap();
            let later = send(&mut state, room, agent, "after resume", 121_000);
            assert_eq!(
                state.expire_stalled_queued_requests(121_020).unwrap(),
                vec![head]
            );
            assert_eq!(state.queued_requests(agent), &[later]);
        }
    }
}

#[test]
fn rejected_retry_that_becomes_uncertain_is_never_expired_or_replayed_as_queued() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let (mut state, room, agent, head) = fixture(provider);
        let later = send(&mut state, room, agent, "later", 120_000);
        state.begin_submission(head, "launch", 5).unwrap();
        state
            .record_submission(
                head,
                SubmissionOutcome::Uncertain {
                    message: "timeout".into(),
                },
            )
            .unwrap();
        state
            .observe_status(agent, RuntimeStatus::Working, 121_019)
            .unwrap();
        assert!(state
            .expire_stalled_queued_requests(121_020)
            .unwrap()
            .is_empty());
        assert_eq!(state.agent(agent).unwrap().current_request, Some(head));
        assert_eq!(state.queued_requests(agent), &[later]);
        assert!(state.request(head).unwrap().uncertain_outcome);
    }
}
