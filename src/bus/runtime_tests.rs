use super::*;
use crate::api::schema::{Method, ResponseResult};
use crate::bus::{
    callbacks, io,
    store::JsonStore,
    transport::{Transport, TransportError},
};
use serde_json::json;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct ScriptedRoomAdapter {
    calls: Arc<
        Mutex<
            Vec<(
                std::thread::ThreadId,
                crate::bus::orchestrator::ModelRequest,
            )>,
        >,
    >,
    agent: AgentId,
}

impl crate::bus::orchestrator::ModelAdapter for ScriptedRoomAdapter {
    fn complete(
        &self,
        request: crate::bus::orchestrator::ModelRequest,
        _credential: crate::bus::credentials::CredentialGeneration,
        _cancel: crate::bus::orchestrator::CancellationToken,
    ) -> impl std::future::Future<
        Output = Result<
            crate::bus::orchestrator::ModelResponse,
            crate::bus::orchestrator::ProviderError,
        >,
    > + Send {
        let calls = Arc::clone(&self.calls);
        let agent = self.agent;
        async move {
            let mut calls = calls.lock().unwrap();
            calls.push((std::thread::current().id(), request));
            let tool_calls = if calls.len() == 1 {
                vec![
                    crate::bus::orchestrator::ProviderToolCall {
                        id: "read-content".into(),
                        name: "read_content".into(),
                        arguments: json!({"kind":"system","name":"system"}),
                    },
                    crate::bus::orchestrator::ProviderToolCall {
                        id: "persist-draft".into(),
                        name: "persist_workflow_draft".into(),
                        arguments: json!({
                            "draft_id":null,
                            "expected_revision":null,
                            "markdown":"# Recovery SOP\n\n## Observe\n\nRead current facts.\n"
                        }),
                    },
                    crate::bus::orchestrator::ProviderToolCall {
                        id: "read-draft".into(),
                        name: "read_workflow_draft".into(),
                        arguments: json!({
                            "draft_id":"draft-0000000000000001",
                            "max_bytes":4096
                        }),
                    },
                    crate::bus::orchestrator::ProviderToolCall {
                        id: "delegate".into(),
                        name: "send_message".into(),
                        arguments: json!({
                            "to":{"agent":agent.0},
                            "text":"Implement the bounded technical change",
                            "work_id":11
                        }),
                    },
                    crate::bus::orchestrator::ProviderToolCall {
                        id: "inspect-delegated-work".into(),
                        name: "inspect_work".into(),
                        arguments: json!({"work_id":11}),
                    },
                ]
            } else {
                Vec::new()
            };
            Ok(crate::bus::orchestrator::ModelResponse {
                content: None,
                tool_calls,
            })
        }
    }
}

#[test]
fn room_orchestrator_core_worker_drives_model_loop_and_settles_content_and_workflow_operations() {
    let (mut worker, _agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let adapter = ScriptedRoomAdapter {
        calls: Arc::clone(&calls),
        agent: _agent,
    };
    worker
        .install_test_orchestrator(adapter, dir.clone())
        .unwrap();
    let mut settings = crate::bus::settings::BusSettings::default();
    settings.orchestrator.system_prompt_override =
        Some("Live custom system prompt\nKeep permissions intact.".into());
    crate::bus::settings::save(&dir.join("settings.json"), &settings).unwrap();
    for _ in 0..20 {
        worker.drive_orchestrator().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        calls.lock().unwrap().is_empty(),
        "an installed model loop must remain dormant until a room fact changes"
    );
    let mut state = worker.state.clone();
    state
        .orchestrator_state_mut()
        .grant(
            room,
            crate::bus::orchestrator::ParticipantId::Human,
            crate::bus::orchestrator::ParticipantId::Orchestrator,
            crate::bus::orchestrator::Capability::PersistWorkflowDraft,
        )
        .unwrap();
    state
        .orchestrator_state_mut()
        .grant(
            room,
            crate::bus::orchestrator::ParticipantId::Human,
            crate::bus::orchestrator::ParticipantId::Orchestrator,
            crate::bus::orchestrator::Capability::Coordinate,
        )
        .unwrap();
    state
        .orchestrator_state_mut()
        .grant(
            room,
            crate::bus::orchestrator::ParticipantId::Human,
            crate::bus::orchestrator::ParticipantId::Orchestrator,
            crate::bus::orchestrator::Capability::InspectRoom,
        )
        .unwrap();
    worker.save(state).unwrap();
    let (events, _) = mpsc::channel();
    worker
        .command(
            BusCommand::SetDraftText(room, "Inspect the room and adapt the SOP".into()),
            &events,
        )
        .unwrap();
    worker
        .command(BusCommand::SetRecipients(room, [_agent].into()), &events)
        .unwrap();
    worker.command(BusCommand::Submit(room), &events).unwrap();
    assert!(worker
        .state
        .orchestrator_state()
        .context(room)
        .facts
        .iter()
        .any(|fact| matches!(
            fact,
            crate::bus::orchestrator::JournalFact::HumanMessage { .. }
        )));

    let worker_thread = std::thread::current().id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while calls.lock().unwrap().len() < 2 && std::time::Instant::now() < deadline {
        worker.drive_orchestrator().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        2,
        "tool settlement must wake the model with results"
    );
    assert_ne!(
        calls[0].0, worker_thread,
        "provider work must stay off Worker"
    );
    assert_eq!(
        calls[0].1.messages[0].content,
        "Live custom system prompt\nKeep permissions intact."
    );
    assert!(calls[0].1.messages.iter().any(|message| message
        .content
        .contains("Remain high-level and process-focused")));
    assert!(calls[1]
        .1
        .messages
        .iter()
        .any(|message| message.content.contains("read_content")
            && message.content.contains("Live custom system prompt")
            && message.content.contains("persist_workflow_draft")
            && message.content.contains("# Recovery SOP")
            && message.content.contains("provider_request_settlement")));
    drop(calls);

    let draft_dir = dir.join(".bus/temp").join(room.0.to_string());
    let drafts = std::fs::read_dir(&draft_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(
        std::fs::read_to_string(drafts[0].path()).unwrap(),
        "# Recovery SOP\n\n## Observe\n\nRead current facts.\n"
    );
    let delegated = worker
        .state
        .requests()
        .find(|request| request.prompt.text == "Implement the bounded technical change")
        .expect("model-authored coding-agent assignment must be durably queued");
    assert_eq!(
        delegated.prompt.author,
        crate::bus::orchestrator::ParticipantId::Orchestrator
    );
    assert_eq!(
        delegated.prompt.work_id,
        Some(crate::bus::orchestrator::WorkId(11))
    );
    assert!(worker
        .state
        .orchestrator_state()
        .operations()
        .any(|operation| operation.kind == "persist_workflow_draft"
            && operation.phase == crate::bus::orchestrator::OperationPhase::Settled
            && matches!(
                operation.result,
                Some(crate::bus::orchestrator::OperationResult::Applied { .. })
            )));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_approved_orchestrator_abandon_idle_request_matches_human_recovery() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    worker
        .install_test_orchestrator(ScriptedRoomAdapter { calls, agent }, dir.clone())
        .unwrap();
    let mut state = worker.state.clone();
    state
        .orchestrator_state_mut()
        .grant(
            room,
            crate::bus::orchestrator::ParticipantId::Human,
            crate::bus::orchestrator::ParticipantId::Orchestrator,
            crate::bus::orchestrator::Capability::AbandonIdleRequest,
        )
        .unwrap();
    let first = state
        .submit_message(
            room,
            Draft {
                text: "historically wedged".into(),
                files: Vec::new(),
                recipient_ids: [agent].into_iter().collect(),
            },
            1,
        )
        .unwrap()[0];
    let queued = state
        .submit_message(
            room,
            Draft {
                text: "must remain queued".into(),
                files: Vec::new(),
                recipient_ids: [agent].into_iter().collect(),
            },
            2,
        )
        .unwrap()[0];
    state.begin_submission(first, "launch", 3).unwrap();
    state
        .record_submission(
            first,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 4).unwrap();
    worker.save(state).unwrap();
    let semantic_revision = worker.state.orchestrator_state().wake_revision(room);

    let result = worker
        .settle_test_orchestrator_operation(
            room,
            crate::bus::orchestrator::RoomOperation::AbandonIdleRequest {
                request_id: first,
                expected_agent: agent,
                expected_incarnation: 1,
                expected_semantic_revision: semantic_revision,
                expected_turn: Some("turn-1".into()),
            },
        )
        .unwrap();

    assert_eq!(result["receipt"]["stage"], "abandoned");
    assert_eq!(
        worker.state.request(first).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(worker.state.queued_requests(agent), &[queued]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[path = "runtime_resume_tests.rs"]
mod resume_tests;

#[test]
fn delivery_logs_explain_queued_hook_gate_once_without_prompt_contents() {
    let capture = crate::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
        let mut value = serde_json::to_value(&worker.state).unwrap();
        value["agents"][agent.0.to_string()]["hook_setup_confirmed"] = json!(false);
        worker.save(serde_json::from_value(value).unwrap()).unwrap();
        let (events, _) = mpsc::channel();
        worker
            .command(
                BusCommand::SetDraftText(room, "PRIVATE_PROMPT".into()),
                &events,
            )
            .unwrap();
        worker
            .command(BusCommand::SetRecipients(room, [agent].into()), &events)
            .unwrap();
        worker.command(BusCommand::Submit(room), &events).unwrap();
        for _ in 0..3 {
            worker.submit_ready().unwrap();
        }
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(
            worker.state.requests().next().unwrap().phase,
            RequestPhase::Queued
        );
        let logs = capture.text();
        assert!(logs.contains("bus.message.queued"), "{logs}");
        assert_eq!(logs.matches("bus.delivery.wait").count(), 1, "{logs}");
        assert!(logs.contains("hook_setup_unconfirmed"), "{logs}");
        assert!(logs.contains("request_id=4"), "{logs}");
        assert!(!logs.contains("PRIVATE_PROMPT"), "{logs}");
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn delivery_logs_correlate_submission_callback_and_persisted_reply() {
    let capture = crate::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        let request = queue(&mut worker, room, agent, "PRIVATE_PROMPT");
        worker.submit_ready().unwrap(); // Fake transport's unknown response is deliberately uncertain.
        record(&dir, Provider::Codex, json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"PRIVATE_PROMPT"}));
        record(&dir, Provider::Codex, json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"PRIVATE_REPLY"}));
        worker.consume_callbacks(agent, &dir.join("callbacks/launch")).unwrap();
        let mut state = worker.state.clone();
        state.observe_status(agent, RuntimeStatus::Idle, 100).unwrap();
        worker.save(state).unwrap();
        assert_eq!(worker.state.request(request).unwrap().phase, RequestPhase::Completed);
        let logs = capture.text();
        for event in ["bus.delivery.start", "bus.delivery.result", "bus.callback.observed", "bus.callback.correlated", "bus.reply.persisted"] {
            assert!(logs.contains(event), "missing {event}: {logs}");
        }
        assert!(logs.contains("uncertain"), "{logs}");
        assert!(logs.contains("request_id=4"), "{logs}");
        assert!(!logs.contains("PRIVATE_PROMPT") && !logs.contains("PRIVATE_REPLY"), "{logs}");
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn provider_start_hooks_bind_when_terminal_trims_trailing_prompt_whitespace() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
        let request = queue(&mut worker, room, agent, "round trip payload ");
        worker.submit_ready().unwrap();
        let callback = match provider {
            Provider::ClaudeCode => json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "session",
                "prompt_id": "turn",
                "prompt": "round trip payload"
            }),
            Provider::Codex => json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "session",
                "turn_id": "turn",
                "prompt": "round trip payload"
            }),
            Provider::Cursor => json!({
                "hook_event_name": "beforeSubmitPrompt",
                "conversation_id": "session",
                "generation_id": "turn",
                "prompt": "round trip payload"
            }),
        };
        record(&dir, provider, callback);
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();

        assert!(
            worker.state.request(request).unwrap().trusted_start_bound,
            "terminal-trimmed prompt must bind for {provider:?}"
        );
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

struct FakeTransport {
    replies: VecDeque<Result<ResponseResult, TransportError>>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    state_path: PathBuf,
}

#[test]
fn delivery_logs_do_not_claim_reply_persisted_when_storage_fails() {
    let capture = crate::logging::test_capture::Capture::default();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "PRIVATE_PROMPT");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"PRIVATE_PROMPT"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"PRIVATE_REPLY"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let mut state = worker.state.clone();
    state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    std::fs::rename(worker.store.path(), dir.join("original-state.json")).unwrap();
    std::fs::create_dir(worker.store.path()).unwrap();
    capture.run(|| {
        assert!(worker.save(state).is_err());
    });
    let logs = capture.text();
    assert!(logs.contains("bus.storage.failed"), "{logs}");
    assert!(!logs.contains("bus.reply.persisted"), "{logs}");
    assert!(!logs.contains("PRIVATE_"), "{logs}");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

impl Transport for FakeTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.calls
            .lock()
            .unwrap()
            .push(crate::api::api_method_name(&method));
        if let Method::PaneCloseIfIdentity(params) = &method {
            let state = JsonStore::new(self.state_path.clone())
                .load()
                .unwrap()
                .unwrap();
            let agent = state
                .agents()
                .find(|agent| {
                    agent.runtime_identity.terminal_id.as_deref()
                        == Some(&params.expected_terminal_id)
                })
                .unwrap();
            assert!(
                agent.deletion_pending,
                "delivery must stop durably before terminal shutdown"
            );
            assert_eq!(
                params.pane_id,
                agent.runtime_identity.pane_id.clone().unwrap()
            );
            assert_eq!(
                params.expected_session_id,
                agent.runtime_identity.session_id
            );
            assert_eq!(
                params.expected_managed_name,
                format!("bus-r{}-a{}", agent.room_id.0, agent.id.0)
            );
        }
        if matches!(
            method,
            Method::AgentPromptIfIdle(_) | Method::AgentPromptIfUnbound(_)
        ) {
            let state = JsonStore::new(self.state_path.clone())
                .load()
                .unwrap()
                .unwrap();
            assert!(state
                .requests()
                .any(|r| r.phase == RequestPhase::Submitting));
        }
        self.replies
            .pop_front()
            .unwrap_or(Ok(ResponseResult::Ok {}))
    }
}

fn fixture(
    provider: Provider,
    replies: Vec<Result<ResponseResult, TransportError>>,
) -> (
    Worker,
    AgentId,
    RoomId,
    PathBuf,
    Arc<Mutex<Vec<&'static str>>>,
) {
    let dir = std::env::temp_dir().join(format!(
        "bus-worker-{}-{}-{}",
        std::process::id(),
        io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeTransport {
        replies: replies.into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut worker = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    let mut state = worker.state.clone();
    let room = state.create_room("room").unwrap();
    let agent = state
        .create_agent(room, "author", provider, dir.clone(), None)
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
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    worker.save(state).unwrap();
    callbacks::initialize(
        &dir.join("callbacks/launch"),
        &callbacks::Manifest {
            agent_id: agent,
            provider,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    (worker, agent, room, dir, calls)
}

#[test]
fn room_orchestrator_core_worker_confirms_exact_room_brief_with_durable_idempotent_receipt() {
    let (mut worker, _agent, room, dir, _calls) = fixture(Provider::Codex, vec![]);
    let proposal = RoomBrief {
        goal: "Ship the reviewed room workflow".into(),
        non_goals: "Do not implement repository changes in the orchestrator".into(),
        revision: 3,
        approved_revision: 0,
        locked: false,
    };
    let proposal_digest = proposal.proposal_digest();
    let mut state = worker.state.clone();
    state.set_room_brief(room, proposal).unwrap();
    worker.save(state).unwrap();

    let stale_state = worker.state.clone();
    assert!(worker
        .command(
            BusCommand::ConfirmRoomBriefProposal(
                crate::bus::orchestrator::ConfirmRoomBriefProposal {
                    room_id: room,
                    expected_developer: crate::bus::orchestrator::ParticipantId::Human,
                    expected_proposal_revision: 2,
                    expected_proposal_digest: proposal_digest.clone(),
                },
            ),
            &mpsc::channel().0,
        )
        .is_err());
    assert_eq!(worker.state, stale_state);

    let command = crate::bus::orchestrator::ConfirmRoomBriefProposal {
        room_id: room,
        expected_developer: crate::bus::orchestrator::ParticipantId::Human,
        expected_proposal_revision: 3,
        expected_proposal_digest: proposal_digest.clone(),
    };
    worker
        .command(
            BusCommand::ConfirmRoomBriefProposal(command.clone()),
            &mpsc::channel().0,
        )
        .unwrap();
    let receipt = worker
        .state
        .orchestrator_state()
        .room_brief_confirmation(room)
        .unwrap()
        .clone();
    assert_eq!(
        receipt.developer,
        crate::bus::orchestrator::ParticipantId::Human
    );
    assert_eq!(receipt.approved_revision, 3);
    assert_eq!(receipt.proposal_digest, proposal_digest);
    assert_eq!(
        worker
            .state
            .orchestrator_state()
            .context(room)
            .facts
            .iter()
            .filter(|fact| matches!(
                fact,
                crate::bus::orchestrator::JournalFact::RoomBriefConfirmed {
                    confirmation_id
                } if *confirmation_id == receipt.confirmation_id
            ))
            .count(),
        1
    );
    let locked = &worker.state.room(room).unwrap().brief;
    assert!(locked.locked);
    assert_eq!(locked.approved_revision, locked.revision);
    let mut forbidden_revision = worker.state.clone();
    assert!(forbidden_revision
        .set_room_brief(
            room,
            RoomBrief {
                goal: "Replace the confirmed goal".into(),
                non_goals: String::new(),
                revision: 4,
                approved_revision: 0,
                locked: false,
            },
        )
        .is_err());

    let confirmed_state = worker.state.clone();
    worker
        .command(
            BusCommand::ConfirmRoomBriefProposal(command.clone()),
            &mpsc::channel().0,
        )
        .unwrap();
    assert_eq!(worker.state, confirmed_state);
    assert_eq!(
        worker
            .state
            .orchestrator_state()
            .room_brief_confirmation(room),
        Some(&receipt)
    );
    let mut conflicting_retry = command.clone();
    conflicting_retry.expected_proposal_digest = "conflicting-proposal".into();
    assert!(worker
        .command(
            BusCommand::ConfirmRoomBriefProposal(conflicting_retry),
            &mpsc::channel().0,
        )
        .is_err());
    assert_eq!(worker.state, confirmed_state);

    drop(worker);
    let mut recovered = Worker::open(
        dir.clone(),
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
            state_path: dir.join("state.json"),
        }),
    )
    .unwrap();
    assert_eq!(
        recovered
            .state
            .orchestrator_state()
            .room_brief_confirmation(room),
        Some(&receipt)
    );
    let recovered_state = recovered.state.clone();
    recovered
        .command(
            BusCommand::ConfirmRoomBriefProposal(command),
            &mpsc::channel().0,
        )
        .unwrap();
    assert_eq!(recovered.state, recovered_state);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_worker_creates_exact_durable_workflow_approval_once() {
    let (mut worker, agent, room, dir, _calls) = fixture(Provider::Codex, vec![]);
    worker
        .install_test_orchestrator(
            ScriptedRoomAdapter {
                calls: Arc::new(Mutex::new(Vec::new())),
                agent,
            },
            dir.clone(),
        )
        .unwrap();
    let store = crate::bus::workflow_drafts::WorkflowDraftStore::new(dir.clone()).unwrap();
    let mut state = worker.state.clone();
    let draft = store
        .persist(
            state.orchestrator_state_mut().workflow_drafts_mut(),
            room,
            crate::bus::orchestrator::WorkflowDraftMutation {
                draft_id: None,
                expected_revision: None,
                workflow_id: Some("release-sop".into()),
                markdown: "# Release SOP\n\n## Verify\n\nRead the exact evidence.\n".into(),
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    let command = store
        .promotion_review(
            worker.state.orchestrator_state().workflow_drafts(),
            room,
            &draft.draft_id,
        )
        .unwrap()
        .approval_command();

    let mut stale = command.clone();
    stale.reviewed_diff_digest = "stale-review".into();
    let before_rejection = worker.state.clone();
    assert!(worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(stale),
            &mpsc::channel().0,
        )
        .is_err());
    assert_eq!(durable(&worker.state), durable(&before_rejection));
    assert_eq!(
        worker
            .state
            .orchestrator_state()
            .workflow_drafts()
            .approvals()
            .count(),
        0
    );

    let mut wrong_developer = command.clone();
    wrong_developer.expected_developer = crate::bus::orchestrator::ParticipantId::Orchestrator;
    assert!(worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(wrong_developer),
            &mpsc::channel().0,
        )
        .is_err());
    assert_eq!(durable(&worker.state), durable(&before_rejection));

    let standard_dir = dir.join(".bus/standard");
    std::fs::create_dir_all(&standard_dir).unwrap();
    std::fs::write(standard_dir.join("release-sop.md"), "# Changed base\n").unwrap();
    assert!(worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(command.clone()),
            &mpsc::channel().0,
        )
        .is_err());
    assert_eq!(durable(&worker.state), durable(&before_rejection));
    std::fs::remove_file(standard_dir.join("release-sop.md")).unwrap();

    let draft_path = dir
        .join(".bus/temp")
        .join(room.0.to_string())
        .join(format!("{}.md", draft.draft_id));
    let draft_markdown = std::fs::read(&draft_path).unwrap();
    std::fs::write(&draft_path, "# Tampered SOP\n\n## Step\n\nChanged.\n").unwrap();
    assert!(worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(command.clone()),
            &mpsc::channel().0,
        )
        .is_err());
    assert_eq!(durable(&worker.state), durable(&before_rejection));
    std::fs::write(draft_path, draft_markdown).unwrap();

    worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(command.clone()),
            &mpsc::channel().0,
        )
        .unwrap();
    let approval = worker
        .state
        .orchestrator_state()
        .workflow_drafts()
        .approvals()
        .next()
        .unwrap()
        .clone();
    assert_eq!(
        approval.developer,
        crate::bus::orchestrator::ParticipantId::Human
    );
    assert_eq!(approval.content_digest, command.content_digest);
    assert_eq!(approval.standard_base_digest, command.standard_base_digest);
    assert_eq!(approval.reviewed_diff_digest, command.reviewed_diff_digest);
    assert_eq!(
        worker
            .state
            .orchestrator_state()
            .context(room)
            .facts
            .iter()
            .filter(|fact| matches!(
                fact,
                crate::bus::orchestrator::JournalFact::WorkflowApprovalCreated {
                    approval_id
                } if approval_id == &approval.approval_id
            ))
            .count(),
        1
    );

    let approved_state = worker.state.clone();
    worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(command.clone()),
            &mpsc::channel().0,
        )
        .unwrap();
    assert_eq!(worker.state, approved_state);
    assert_eq!(
        worker
            .state
            .orchestrator_state()
            .workflow_drafts()
            .approvals()
            .count(),
        1
    );

    drop(worker);
    let mut recovered = Worker::open(
        dir.clone(),
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
            state_path: dir.join("state.json"),
        }),
    )
    .unwrap();
    assert_eq!(
        recovered
            .state
            .orchestrator_state()
            .workflow_drafts()
            .approval(&approval.approval_id),
        Some(&approval)
    );
    recovered
        .install_test_orchestrator(
            ScriptedRoomAdapter {
                calls: Arc::new(Mutex::new(Vec::new())),
                agent,
            },
            dir.clone(),
        )
        .unwrap();
    let recovered_state = recovered.state.clone();
    recovered
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(command),
            &mpsc::channel().0,
        )
        .unwrap();
    assert_eq!(recovered.state, recovered_state);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

fn durable(state: &BusState) -> serde_json::Value {
    serde_json::to_value(state).unwrap()
}

fn grant_orchestrator(
    worker: &mut Worker,
    room: RoomId,
    capabilities: &[crate::bus::orchestrator::Capability],
) {
    let mut state = worker.state.clone();
    for capability in capabilities {
        state
            .orchestrator_state_mut()
            .grant(
                room,
                crate::bus::orchestrator::ParticipantId::Human,
                crate::bus::orchestrator::ParticipantId::Orchestrator,
                *capability,
            )
            .unwrap();
    }
    worker.save(state).unwrap();
}

fn reopen(dir: &std::path::Path) -> Worker {
    Worker::open(
        dir.to_owned(),
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
            state_path: dir.join("state.json"),
        }),
    )
    .unwrap()
}

#[test]
fn room_orchestrator_core_message_orchestrator_persists_exact_body_and_wakes_without_a_request() {
    use crate::bus::orchestrator::{
        HumanOrchestratorMessage, JournalFact, ParticipantId, RoomRecipient,
    };

    let (mut worker, agent, room, dir, transport_calls) = fixture(Provider::Codex, vec![]);
    let (events, _) = mpsc::channel();
    let body = "  Status check:\nwhich work is blocked?  ";
    let message = |room_id: RoomId, body: &str| {
        BusCommand::MessageOrchestrator(HumanOrchestratorMessage {
            room_id,
            body: body.into(),
        })
    };

    let disabled = worker.state.clone();
    assert!(worker
        .command(message(room, body), &events)
        .unwrap_err()
        .contains("disabled"));
    assert_eq!(worker.state, disabled);

    let calls = Arc::new(Mutex::new(Vec::new()));
    worker
        .install_test_orchestrator(
            ScriptedRoomAdapter {
                calls: Arc::clone(&calls),
                agent,
            },
            dir.clone(),
        )
        .unwrap();
    let before = worker.state.clone();
    for rejected in [message(room, " \n\t"), message(RoomId(room.0 + 1000), body)] {
        assert!(worker.command(rejected, &events).is_err());
        assert_eq!(worker.state, before);
    }

    assert!(before.room(room).unwrap().draft.recipient_ids.is_empty());
    let wake_before = before.orchestrator_state().wake_revision(room);
    let transport_before = transport_calls.lock().unwrap().len();
    worker.command(message(room, body), &events).unwrap();
    let records = worker
        .state
        .room_messages(room)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].message.author, ParticipantId::Human);
    assert_eq!(records[0].message.to, RoomRecipient::Orchestrator);
    assert_eq!(records[0].message.text, body);
    assert_eq!(
        worker.state.orchestrator_state().wake_revision(room),
        wake_before + 1
    );
    assert_eq!(
        worker
            .state
            .orchestrator_state()
            .context(room)
            .facts
            .iter()
            .filter(|fact| matches!(
                fact,
                JournalFact::HumanMessage { message_id } if *message_id == records[0].message_id
            ))
            .count(),
        1
    );
    assert_eq!(worker.state.requests().count(), before.requests().count());
    assert_eq!(
        worker.state.queued_requests(agent),
        before.queued_requests(agent)
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_prompt,
        before.room(room).unwrap().latest_prompt
    );
    assert_eq!(transport_calls.lock().unwrap().len(), transport_before);

    let quoted_body = serde_json::to_string(body).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while calls.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        worker.drive_orchestrator().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        calls.lock().unwrap()[0]
            .1
            .messages
            .iter()
            .any(|message| message.content.contains(&quoted_body)),
        "the Human message wake must carry the exact body into model context"
    );

    drop(worker);
    let recovered = reopen(&dir);
    assert_eq!(
        recovered
            .state
            .room_messages(room)
            .cloned()
            .collect::<Vec<_>>(),
        records
    );
    assert!(
        serde_json::to_string(&recovered.state.orchestrator_state().context(room))
            .unwrap()
            .contains(&quoted_body)
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_orchestrator_human_addressed_message_is_a_room_message_not_a_request() {
    use crate::bus::orchestrator::{
        Capability, ParticipantId, RoomMessage, RoomOperation, RoomRecipient,
    };

    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker
        .install_test_orchestrator(
            ScriptedRoomAdapter {
                calls: Arc::new(Mutex::new(Vec::new())),
                agent,
            },
            dir.clone(),
        )
        .unwrap();
    grant_orchestrator(&mut worker, room, &[Capability::Coordinate]);
    let before = worker.state.clone();
    let message = |to, text: &str| {
        RoomOperation::SendMessage(RoomMessage {
            author: ParticipantId::Orchestrator,
            to,
            text: text.into(),
            work: None,
        })
    };

    assert!(worker
        .settle_test_orchestrator_operation(
            room,
            message(RoomRecipient::Orchestrator, "note to self")
        )
        .is_err());
    assert_eq!(worker.state.room_messages(room).count(), 0);

    let settled = worker
        .settle_test_orchestrator_operation(
            room,
            message(RoomRecipient::Human, "Please verify the gesture manually."),
        )
        .unwrap();
    let records = worker
        .state
        .room_messages(room)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].message.author, ParticipantId::Orchestrator);
    assert_eq!(records[0].message.to, RoomRecipient::Human);
    assert_eq!(
        records[0].message.text,
        "Please verify the gesture manually."
    );
    assert_eq!(settled["receipt"]["message_id"], records[0].message_id.0);
    assert_eq!(worker.state.requests().count(), before.requests().count());
    assert_eq!(
        worker.state.queued_requests(agent),
        before.queued_requests(agent)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_worker_publishes_typed_workflow_promotion_review_for_exact_approval() {
    use crate::bus::orchestrator::{
        Capability, ParticipantId, RoomOperation, WorkflowDraftMutation,
    };

    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let adapter = || ScriptedRoomAdapter {
        calls: Arc::new(Mutex::new(Vec::new())),
        agent,
    };
    worker
        .install_test_orchestrator(adapter(), dir.clone())
        .unwrap();
    grant_orchestrator(&mut worker, room, &[Capability::PersistWorkflowDraft]);
    let (events, _) = mpsc::channel();
    let persist = |draft_id: Option<&str>,
                   expected_revision: Option<u64>,
                   workflow_id: Option<&str>,
                   markdown: &str| {
        RoomOperation::PersistWorkflowDraft(WorkflowDraftMutation {
            draft_id: draft_id.map(Into::into),
            expected_revision,
            workflow_id: workflow_id.map(Into::into),
            markdown: markdown.into(),
        })
    };

    worker
        .settle_test_orchestrator_operation(
            room,
            persist(None, None, None, "# Local SOP\n\n## Step\n\nRoom only.\n"),
        )
        .unwrap();
    assert!(worker.state.workflow_promotion_reviews(room).is_empty());

    let created = worker
        .settle_test_orchestrator_operation(
            room,
            persist(
                None,
                None,
                Some("release-sop"),
                "# Release SOP\n\n## Verify\n\nRead evidence.\n",
            ),
        )
        .unwrap();
    let draft_id = created["receipt"]["draft_id"].as_str().unwrap().to_owned();
    let first = worker.state.workflow_promotion_reviews(room).to_vec();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].draft_id, draft_id);
    assert_eq!(first[0].draft_revision, 1);
    assert_eq!(first[0].workflow_id, "release-sop");
    assert_eq!(first[0].expected_developer, ParticipantId::Human);
    assert_eq!(first[0].standard_base_digest, None);
    assert!(first[0].rendered_diff.contains("+# Release SOP\n"));

    let retarget = worker.settle_test_orchestrator_operation(
        room,
        persist(
            Some(&draft_id),
            Some(1),
            Some("other-sop"),
            "# Release SOP\n\n## Verify\n\nRetargeted.\n",
        ),
    );
    assert!(retarget.unwrap_err().contains("TargetImmutable"));
    assert_eq!(
        worker.state.workflow_promotion_reviews(room),
        first.as_slice()
    );

    let displayed = first[0].approval_command();
    worker
        .settle_test_orchestrator_operation(
            room,
            persist(
                Some(&draft_id),
                Some(1),
                None,
                "# Release SOP\n\n## Verify\n\nRead revised evidence.\n",
            ),
        )
        .unwrap();
    let revised = worker.state.workflow_promotion_reviews(room).to_vec();
    assert_eq!(revised[0].draft_revision, 2);
    assert_eq!(revised[0].workflow_id, "release-sop");
    let before_stale = durable(&worker.state);
    assert!(worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(displayed),
            &events
        )
        .unwrap_err()
        .contains("StaleApproval"));
    assert_eq!(durable(&worker.state), before_stale);

    let standard = dir.join(".bus/standard");
    std::fs::create_dir_all(&standard).unwrap();
    std::fs::write(standard.join("release-sop.md"), "# Release SOP\n\n## Old\n").unwrap();
    let before_base_change = durable(&worker.state);
    assert!(worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(revised[0].approval_command()),
            &events
        )
        .unwrap_err()
        .contains("StaleApproval"));
    assert_eq!(durable(&worker.state), before_base_change);
    let current = worker.state.workflow_promotion_reviews(room)[0].clone();
    assert!(current.standard_base_digest.is_some());
    assert!(current.rendered_diff.contains("-## Old\n"));

    worker
        .command(
            BusCommand::CreateDeveloperWorkflowApproval(current.approval_command()),
            &events,
        )
        .unwrap();
    let approval = worker.state.developer_workflow_approvals(room)[0].clone();
    assert_eq!(worker.state.developer_workflow_approvals(room).len(), 1);
    assert_eq!(
        (
            approval.draft_id.as_str(),
            approval.draft_revision,
            approval.content_digest.as_str(),
            approval.workflow_id.as_str(),
            approval.standard_base_digest.as_deref(),
            approval.reviewed_diff_digest.as_str(),
        ),
        (
            current.draft_id.as_str(),
            current.draft_revision,
            current.content_digest.as_str(),
            current.workflow_id.as_str(),
            current.standard_base_digest.as_deref(),
            current.reviewed_diff_digest.as_str(),
        )
    );

    drop(worker);
    let mut recovered = reopen(&dir);
    recovered
        .install_test_orchestrator(adapter(), dir.clone())
        .unwrap();
    assert_eq!(
        recovered.state.workflow_promotion_reviews(room),
        std::slice::from_ref(&current)
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_worker_rejects_displayed_brief_digest_after_same_revision_edit() {
    let (mut worker, _agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let (events, _) = mpsc::channel();
    let brief = |goal: &str| RoomBrief {
        goal: goal.into(),
        non_goals: "No orchestrator coding".into(),
        revision: 3,
        approved_revision: 0,
        locked: false,
    };
    let mut state = worker.state.clone();
    state
        .set_room_brief(room, brief("Ship the reviewed workflow"))
        .unwrap();
    worker.save(state).unwrap();
    let (displayed_revision, displayed_digest) = worker.state.room_brief_proposal(room).unwrap();
    assert_eq!(displayed_revision, 3);

    let mut state = worker.state.clone();
    state
        .set_room_brief(room, brief("Ship a different workflow"))
        .unwrap();
    worker.save(state).unwrap();
    let (current_revision, current_digest) = worker.state.room_brief_proposal(room).unwrap();
    assert_eq!(current_revision, displayed_revision);
    assert_ne!(current_digest, displayed_digest);

    let confirm = |digest: String| {
        BusCommand::ConfirmRoomBriefProposal(crate::bus::orchestrator::ConfirmRoomBriefProposal {
            room_id: room,
            expected_developer: crate::bus::orchestrator::ParticipantId::Human,
            expected_proposal_revision: displayed_revision,
            expected_proposal_digest: digest,
        })
    };
    let before = worker.state.clone();
    assert!(worker.command(confirm(displayed_digest), &events).is_err());
    assert_eq!(worker.state, before);
    assert!(worker.state.room_brief_confirmation(room).is_none());

    worker
        .command(confirm(current_digest.clone()), &events)
        .unwrap();
    assert_eq!(worker.state.room_brief_proposal(room), None);
    assert_eq!(
        worker
            .state
            .room_brief_confirmation(room)
            .unwrap()
            .proposal_digest,
        current_digest
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_test_worker_harness_publishes_real_handler_snapshots() {
    use crate::bus::orchestrator::{
        HumanOrchestratorMessage, RoomOperation, WorkflowDraftMutation,
    };

    let mut harness = super::test_harness::TestWorkerHarness::new();
    let room = harness.room();
    harness
        .command(BusCommand::MessageOrchestrator(HumanOrchestratorMessage {
            room_id: room,
            body: "Where are we?".into(),
        }))
        .unwrap();
    harness
        .orchestrator_operation(RoomOperation::PersistWorkflowDraft(WorkflowDraftMutation {
            draft_id: None,
            expected_revision: None,
            workflow_id: Some("triage-sop".into()),
            markdown: "# Triage SOP\n\n## Observe\n\nRead facts.\n".into(),
        }))
        .unwrap();
    harness
        .orchestrator_operation(RoomOperation::ProposeRoomBrief {
            expected_revision: 0,
            goal: "Triage the room".into(),
            non_goals: "No orchestrator coding".into(),
        })
        .unwrap();
    let snapshot = harness.snapshot();
    assert_eq!(snapshot.state.room_messages(room).count(), 1);
    let proposal = snapshot.state.room_brief_proposal(room).unwrap();
    assert_eq!(proposal.0, 1);
    let review = snapshot.state.workflow_promotion_reviews(room)[0].clone();
    harness
        .command(BusCommand::CreateDeveloperWorkflowApproval(
            review.approval_command(),
        ))
        .unwrap();
    let rejected = harness.command(BusCommand::MessageOrchestrator(HumanOrchestratorMessage {
        room_id: room,
        body: " ".into(),
    }));
    assert!(rejected.is_err());
    let before_restart = harness.snapshot();
    assert!(before_restart.revision > snapshot.revision);
    assert_eq!(before_restart.error, rejected.err());

    harness.restart();
    let restarted = harness.snapshot();
    assert_eq!(restarted.state.room_messages(room).count(), 1);
    assert_eq!(restarted.state.room_brief_proposal(room), Some(proposal));
    assert_eq!(restarted.state.developer_workflow_approvals(room).len(), 1);
    assert_eq!(
        restarted.state.workflow_promotion_reviews(room),
        std::slice::from_ref(&review)
    );
    assert_eq!(
        restarted.state.agent(harness.agent()).unwrap().room_id,
        room
    );
}

#[test]
fn room_orchestrator_core_propose_room_brief_allocates_revisions_without_confirming() {
    use crate::bus::orchestrator::{ConfirmRoomBriefProposal, ParticipantId, RoomOperation};

    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker
        .install_test_orchestrator(
            ScriptedRoomAdapter {
                calls: Arc::new(Mutex::new(Vec::new())),
                agent,
            },
            dir.clone(),
        )
        .unwrap();
    let propose =
        |expected_revision: u64, goal: &str, non_goals: &str| RoomOperation::ProposeRoomBrief {
            expected_revision,
            goal: goal.into(),
            non_goals: non_goals.into(),
        };
    let brief = |worker: &Worker| worker.state.room(room).unwrap().brief.clone();

    let first = worker
        .settle_test_orchestrator_operation(
            room,
            propose(0, "Ship the reviewed workflow", "No orchestrator coding"),
        )
        .unwrap();
    let (revision, digest) = worker.state.room_brief_proposal(room).unwrap();
    assert_eq!(revision, 1);
    assert_eq!(first["receipt"]["revision"], 1);
    assert_eq!(first["receipt"]["proposal_digest"], digest.as_str());
    assert_eq!(
        brief(&worker),
        RoomBrief {
            goal: "Ship the reviewed workflow".into(),
            non_goals: "No orchestrator coding".into(),
            revision: 1,
            approved_revision: 0,
            locked: false,
        }
    );
    assert!(worker.state.room_brief_confirmation(room).is_none());

    for rejected in [
        propose(0, "Stale proposal", "Stale"),
        propose(2, "Future proposal", "Future"),
        propose(1, " \n\t", "No orchestrator coding"),
        propose(1, "Ship", "  "),
    ] {
        let before = brief(&worker);
        assert!(worker
            .settle_test_orchestrator_operation(room, rejected)
            .is_err());
        assert_eq!(brief(&worker), before);
    }

    worker
        .settle_test_orchestrator_operation(
            room,
            propose(1, "Ship the revised workflow", "No orchestrator coding"),
        )
        .unwrap();
    let (revision, digest) = worker.state.room_brief_proposal(room).unwrap();
    assert_eq!(revision, 2);
    assert!(!brief(&worker).locked);
    assert!(worker.state.room_brief_confirmation(room).is_none());

    worker
        .command(
            BusCommand::ConfirmRoomBriefProposal(ConfirmRoomBriefProposal {
                room_id: room,
                expected_developer: ParticipantId::Human,
                expected_proposal_revision: revision,
                expected_proposal_digest: digest,
            }),
            &mpsc::channel().0,
        )
        .unwrap();
    let locked = brief(&worker);
    assert!(locked.locked);
    assert!(worker
        .settle_test_orchestrator_operation(room, propose(2, "Reopen the goal", "Anything"))
        .is_err());
    assert_eq!(brief(&worker), locked);

    drop(worker);
    assert_eq!(reopen(&dir).state.room(room).unwrap().brief, locked);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_room_brief_confirmation_activates_orchestrator_baseline_once() {
    use crate::bus::orchestrator::{
        Capability, ConfirmRoomBriefProposal, OrchestratorCommand, ParticipantId, RoomMessage,
        RoomOperation, RoomQuery, RoomRecipient,
    };

    const BASELINE: [Capability; 7] = [
        Capability::InspectRoom,
        Capability::Coordinate,
        Capability::AbandonIdleRequest,
        Capability::PersistWorkflowDraft,
        Capability::PromoteWorkflowDraft,
        Capability::ApprovePermissionOnce,
        Capability::ManageResourceLease,
    ];
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker
        .install_test_orchestrator(
            ScriptedRoomAdapter {
                calls: Arc::new(Mutex::new(Vec::new())),
                agent,
            },
            dir.clone(),
        )
        .unwrap();
    let authorized = |worker: &Worker| {
        BASELINE.map(|capability| {
            worker.state.orchestrator_state().authorized(
                room,
                ParticipantId::Orchestrator,
                capability,
            )
        })
    };
    let query = || {
        OrchestratorCommand::Query(RoomQuery::WaitForChange {
            after_revision: 0,
            timeout_ms: 1,
        })
    };
    let message = || {
        OrchestratorCommand::Operation(RoomOperation::SendMessage(RoomMessage {
            author: ParticipantId::Orchestrator,
            to: RoomRecipient::Human,
            text: "Please confirm the Room Brief".into(),
            work: None,
        }))
    };
    let lease = || {
        OrchestratorCommand::Operation(RoomOperation::AcquireResource {
            resource: "benchmark".into(),
            ttl_ms: 1,
        })
    };
    let assert_denied = |worker: &mut Worker, command: OrchestratorCommand| {
        let before = worker.state.clone();
        let settled = worker.settle_test_orchestrator_command(room, command);
        assert_eq!(settled["ok"], false, "{settled}");
        assert!(
            settled["error"].as_str().unwrap().contains("capability"),
            "{settled}"
        );
        assert_eq!(worker.state, before);
    };

    assert_eq!(authorized(&worker), [false; 7]);
    for command in [query(), message(), lease()] {
        assert_denied(&mut worker, command);
    }
    let proposed = worker.settle_test_orchestrator_command(
        room,
        OrchestratorCommand::Operation(RoomOperation::ProposeRoomBrief {
            expected_revision: 0,
            goal: "Ship the reviewed workflow".into(),
            non_goals: "No orchestrator coding".into(),
        }),
    );
    assert_eq!(proposed["ok"], true, "{proposed}");
    assert_eq!(authorized(&worker), [false; 7]);
    assert_denied(&mut worker, query());

    let (revision, digest) = worker.state.room_brief_proposal(room).unwrap();
    let confirm = BusCommand::ConfirmRoomBriefProposal(ConfirmRoomBriefProposal {
        room_id: room,
        expected_developer: ParticipantId::Human,
        expected_proposal_revision: revision,
        expected_proposal_digest: digest,
    });
    worker.command(confirm.clone(), &mpsc::channel().0).unwrap();
    assert_eq!(authorized(&worker), [true; 7]);
    let grants = durable(&worker.state)["orchestrator"]["grants"].clone();
    assert_eq!(grants.as_array().unwrap().len(), BASELINE.len());
    assert!(grants.as_array().unwrap().iter().all(|grant| {
        grant["room_id"] == serde_json::to_value(room).unwrap()
            && grant["participant"] == "orchestrator"
            && grant["revision"] == 1
            && grant["active"] == true
    }));
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(durable(&saved)["orchestrator"]["grants"], grants);
    assert!(saved.room_brief_confirmation(room).is_some());

    let confirmed = worker.state.clone();
    worker.command(confirm.clone(), &mpsc::channel().0).unwrap();
    assert_eq!(worker.state, confirmed);
    for command in [query(), message(), lease()] {
        let settled = worker.settle_test_orchestrator_command(room, command);
        assert_eq!(settled["ok"], true, "{settled}");
    }

    drop(worker);
    let mut recovered = reopen(&dir);
    assert_eq!(authorized(&recovered), [true; 7]);
    assert_eq!(durable(&recovered.state)["orchestrator"]["grants"], grants);
    let recovered_state = recovered.state.clone();
    recovered.command(confirm, &mpsc::channel().0).unwrap();
    assert_eq!(recovered.state, recovered_state);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_every_advertised_tool_settles_through_an_implemented_worker_path() {
    use crate::bus::orchestrator::{Capability, ModelToolCall, OrchestratorToolRegistry};

    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker
        .install_test_orchestrator(
            ScriptedRoomAdapter {
                calls: Arc::new(Mutex::new(Vec::new())),
                agent,
            },
            dir.clone(),
        )
        .unwrap();
    grant_orchestrator(
        &mut worker,
        room,
        &[
            Capability::InspectRoom,
            Capability::Coordinate,
            Capability::AbandonIdleRequest,
            Capability::PersistWorkflowDraft,
            Capability::PromoteWorkflowDraft,
            Capability::ManageResourceLease,
            Capability::ApprovePermissionOnce,
        ],
    );
    let registry = OrchestratorToolRegistry;
    for schema in registry.schemas() {
        let arguments = match schema.name {
            "inspect_work" => json!({"work_id": 1}),
            "wait_for_change" => json!({"after_revision": 0, "timeout_ms": 1}),
            "read_agent" => json!({"agent_id": agent.0, "source": "visible", "lines": null}),
            "observe_permission_prompt" => json!({"agent_id": agent.0}),
            "read_workflow_draft" => {
                json!({"draft_id": "draft-0000000000000001", "max_bytes": 16})
            }
            "read_content" => json!({"kind": "system", "name": "system"}),
            "send_message" => json!({"to": "human", "text": "Need a decision", "work_id": null}),
            "propose_room_brief" => json!({
                "expected_revision": 0,
                "goal": "Ship the reviewed workflow",
                "non_goals": "No orchestrator coding"
            }),
            "abandon_idle_request" => json!({
                "request_id": 1,
                "expected_agent": agent.0,
                "expected_incarnation": 1,
                "expected_semantic_revision": 0,
                "expected_turn": null
            }),
            "persist_workflow_draft" => json!({
                "draft_id": null,
                "expected_revision": null,
                "workflow_id": null,
                "markdown": "# Sample SOP\n\n## Step\n\nRead facts.\n"
            }),
            "promote_workflow_draft" => json!({"approval_id": "approval-missing"}),
            "acquire_resource" => json!({"resource": "benchmark", "ttl_ms": 1}),
            "release_resource" => json!({"lease_id": 999}),
            "approve_permission_once" => json!({"fingerprint": "forged", "response": "allow-once"}),
            name => panic!("{name} needs a Worker-path settlement sample"),
        };
        let command = registry
            .decode(ModelToolCall {
                name: schema.name.into(),
                arguments,
            })
            .unwrap();
        let settled = worker.settle_test_orchestrator_command(room, command);
        assert!(
            !settled
                .to_string()
                .contains("unavailable on this runtime surface"),
            "{}: {settled}",
            schema.name
        );
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

fn queue(worker: &mut Worker, room: RoomId, agent: AgentId, text: &str) -> RequestId {
    let mut state = worker.state.clone();
    state.set_draft_text(room, text).unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, 2).unwrap()[0];
    worker.save(state).unwrap();
    request
}

#[test]
fn delete_agent_stops_terminal_before_removing_persisted_work() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "discard me");
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(worker.state.request(request).is_none());
    assert!(worker
        .state
        .room(room)
        .unwrap()
        .draft
        .recipient_ids
        .is_empty());
    assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
    worker.submit_ready().unwrap();
    drop(worker);
    let recovered = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(recovered.agent(agent).is_none());
    assert!(recovered.request(request).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_failure_suspends_delivery_across_restart_and_allows_retry() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "identity changed".into(),
            code: Some("terminal_identity_changed".into()),
            definitely_rejected: true,
        })],
    );
    let request = queue(
        &mut worker,
        room,
        agent,
        "must never send after delete confirmation",
    );
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(worker.state.request(request).is_some());
    worker.submit_ready().unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
    drop(worker);
    let fake = FakeTransport {
        replies: VecDeque::new(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    recovered.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    recovered
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(recovered.state.agent(agent).is_none());
    assert!(recovered.state.request(request).is_none());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_room_partial_close_preserves_retryable_state_and_other_rooms() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![
            Ok(ResponseResult::Ok {}),
            Err(TransportError {
                message: "stop failed".into(),
                code: Some("terminal_stop_failed".into()),
                definitely_rejected: false,
            }),
        ],
    );
    let project = dir.join("project-file.txt");
    std::fs::write(&project, "keep project data").unwrap();
    let other_room = worker.state.create_room("other").unwrap();
    let other_agent = worker
        .state
        .create_agent(other_room, "unrelated", Provider::Codex, dir.clone(), None)
        .unwrap();
    let second = worker
        .state
        .create_agent(room, "second", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker
        .state
        .set_agent_runtime_identity(
            second,
            AgentRuntimeIdentity {
                launch_id: Some("second-launch".into()),
                terminal_id: Some("second-terminal".into()),
                pane_id: Some("second-pane".into()),
                session_id: Some("second-session".into()),
            },
        )
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "remove");
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteRoom(room), &events)
        .is_err());
    assert!(worker.state.room(room).unwrap().deletion_pending);
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(worker.state.agent(second).unwrap().deletion_pending);
    assert!(!worker.state.agent(other_agent).unwrap().deletion_pending);
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 2);
    drop(worker);
    let fake = FakeTransport {
        replies: VecDeque::new(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .command(BusCommand::DeleteRoom(room), &events)
        .unwrap();
    assert!(recovered.state.room(room).is_none());
    assert!(recovered.state.agent(agent).is_none());
    assert!(recovered.state.agent(second).is_none());
    assert!(recovered.state.request(request).is_none());
    assert!(recovered.state.agent(other_agent).is_some());
    assert!(recovered.state.room(other_room).is_some());
    assert_eq!(
        std::fs::read_to_string(project).unwrap(),
        "keep project data"
    );
    assert_eq!(calls.lock().unwrap().len(), 4);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_unknown_launch_identity_fails_closed_without_transport() {
    let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![]);
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("uncertain-launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(calls.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

struct NativeBoundClose {
    session: String,
    calls: Arc<Mutex<Vec<Option<String>>>>,
    state_path: PathBuf,
}

impl Transport for NativeBoundClose {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let Method::PaneCloseIfIdentity(params) = method else {
            panic!("Deletion must not report native sessions or deliver prompts: {method:?}");
        };
        let saved = JsonStore::new(self.state_path.clone())
            .load()
            .unwrap()
            .unwrap();
        let saved_agent = saved.agents().next().unwrap();
        assert!(saved_agent.deletion_pending);
        assert_eq!(
            saved_agent.runtime_identity.session_id,
            params.expected_session_id
        );
        self.calls
            .lock()
            .unwrap()
            .push(params.expected_session_id.clone());
        if params.expected_session_id.as_deref() == Some(&self.session) {
            Ok(ResponseResult::Ok {})
        } else {
            Err(TransportError {
                message: "terminal_identity_changed".into(),
                code: Some("terminal_identity_changed".into()),
                definitely_rejected: true,
            })
        }
    }
}

#[test]
fn deletion_retry_reconciles_launch_attested_initial_session_without_callbacks_or_delivery() {
    for provider in [Provider::ClaudeCode, Provider::Cursor, Provider::Codex] {
        let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        queue(&mut worker, room, agent, "never deliver");
        let calls = Arc::new(Mutex::new(Vec::new()));
        worker.transport = Box::new(NativeBoundClose {
            session: "native-session".into(),
            calls: Arc::clone(&calls),
            state_path: dir.join("state.json"),
        });
        let (events, _) = mpsc::channel();
        assert!(worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .is_err());
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"sessionStart"} else {"SessionStart"}, "session_id":"native-session", "conversation_id":"native-session"}),
        );
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"beforeSubmitPrompt"} else {"UserPromptSubmit"}, "session_id":"native-session", "conversation_id":"native-session", "prompt_id":"p", "turn_id":"p", "generation_id":"p", "prompt":"never deliver"}),
        );
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"afterAgentResponse"} else {"Stop"}, "session_id":"native-session", "conversation_id":"native-session", "prompt_id":"p", "turn_id":"p", "generation_id":"p", "last_assistant_message":"never publish", "text":"never publish"}),
        );
        worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap();
        assert!(worker.state.agent(agent).is_none());
        assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
        assert_eq!(worker.state.requests().count(), 0);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![None, None, Some("native-session".into())]
        );
        assert_eq!(
            callbacks::records(&dir.join("callbacks/launch"))
                .unwrap()
                .len(),
            3
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deletion_initial_session_reconciliation_rejects_conflicting_launch_sessions() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.state.prepare_delete_agent(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    for session in ["first", "second"] {
        record(
            &dir,
            Provider::ClaudeCode,
            json!({"hook_event_name":"SessionStart", "session_id":session}),
        );
    }
    assert!(worker
        .reconcile_deleting_initial_session(agent)
        .unwrap_err()
        .contains("Conflicting"));
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .session_id
        .is_none());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deletion_never_rebinds_a_known_session_from_a_later_session_start() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(NativeBoundClose {
        session: "rebound".into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    });
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart", "session_id":"rebound"}),
    );
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("session")
    );
    assert_eq!(*calls.lock().unwrap(), vec![Some("session".into())]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsupported_native_close_requires_server_restart_and_never_falls_back() {
    for code in ["unknown_method", "invalid_request"] {
        let (mut worker, agent, _, dir, calls) = fixture(
            Provider::Codex,
            vec![Err(TransportError {
                message: "RAW_API_DETAIL unknown_method: pane.close_if_identity; expected one of many API variants".repeat(20),
                code: Some(code.into()),
                definitely_rejected: true,
            })],
        );
        let (events, _) = mpsc::channel();
        let error = worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap_err();
        assert!(
            error.contains("Update and restart the Bus server"),
            "{error}"
        );
        assert!(error.len() < 220, "user-facing error must remain compact");
        assert!(!error.contains("RAW_API_DETAIL"));
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deletion_transport_details_stay_out_of_user_errors() {
    for result in [
        Err(TransportError {
            message: "RAW_API_DETAIL terminal driver details".repeat(100),
            code: Some("terminal_stop_failed".into()),
            definitely_rejected: false,
        }),
        Ok(ResponseResult::AgentList { agents: vec![] }),
    ] {
        let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![result]);
        let (events, _) = mpsc::channel();
        let error = worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap_err();
        assert!(error.len() < 220, "{error}");
        assert!(
            !error.contains("RAW_API_DETAIL") && !error.contains("AgentList"),
            "{error}"
        );
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn successful_deletion_retry_clears_previous_worker_snapshot_error() {
    for delete_room in [false, true] {
        let (worker, agent, room, dir, _) = fixture(
            Provider::Codex,
            vec![
                Err(TransportError {
                    message: "temporary stop failure".into(),
                    code: Some("terminal_stop_failed".into()),
                    definitely_rejected: true,
                }),
                Ok(ResponseResult::Ok {}),
            ],
        );
        let deletion = if delete_room {
            BusCommand::DeleteRoom(room)
        } else {
            BusCommand::DeleteAgent(agent)
        };
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (commands, receiver) = mpsc::sync_channel(256);
        let (events, received) = mpsc::channel();
        commands.send((1, deletion.clone())).unwrap();
        commands.send((2, deletion)).unwrap();
        commands.send((3, BusCommand::Shutdown)).unwrap();
        worker.run(receiver, events, Arc::clone(&snapshots));
        let outcomes: Vec<_> = received
            .try_iter()
            .filter_map(|event| match event {
                BusEvent::CommandFinished { result, .. } => Some(result),
                _ => None,
            })
            .collect();
        assert!(outcomes[0].is_err());
        assert!(outcomes[1].is_ok());
        assert!(snapshots.lock().unwrap().error.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn delete_last_room_stays_empty_after_restart_and_ignores_late_callbacks() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "deleted request");
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteRoom(room), &events)
        .unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"SessionStart", "session_id":"late-session"}),
    );
    drop(worker);
    let fake = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![] })].into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered.tick().unwrap();
    assert_eq!(recovered.state.rooms().count(), 0);
    assert_eq!(recovered.state.agents().count(), 0);
    assert_eq!(recovered.state.requests().count(), 0);
    assert!(!recovered.state.is_pristine());
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["pane.close_if_identity", "agent.list"]
    );
    let new_room = recovered.state.create_room("new room").unwrap();
    assert!(new_room.0 > agent.0);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn queued_delete_is_applied_before_background_delivery_of_earlier_submit() {
    struct ReadyTransport {
        calls: Arc<Mutex<Vec<&'static str>>>,
        delete_during_poll: Option<(mpsc::SyncSender<(u64, BusCommand)>, AgentId)>,
    }
    impl Transport for ReadyTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            self.calls
                .lock()
                .unwrap()
                .push(crate::api::api_method_name(&method));
            if matches!(method, Method::AgentList(_)) {
                if let Some((commands, agent)) = self.delete_during_poll.take() {
                    commands.send((2, BusCommand::DeleteAgent(agent))).unwrap();
                    commands.send((3, BusCommand::Shutdown)).unwrap();
                }
                let info = serde_json::from_value(json!({
                    "terminal_id":"terminal", "agent":"codex", "agent_status":"idle",
                    "agent_session":{"source":"herdr:codex","agent":"codex","kind":"id","value":"session"},
                    "workspace_id":"workspace", "tab_id":"tab", "pane_id":"pane",
                    "focused":false, "interactive_ready":true, "revision":1
                })).unwrap();
                Ok(ResponseResult::AgentList { agents: vec![info] })
            } else {
                Ok(ResponseResult::Ok {})
            }
        }
    }
    for during_poll in [false, true] {
        let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
        let (commands, receiver) = mpsc::sync_channel(256);
        worker.transport = Box::new(ReadyTransport {
            calls: Arc::clone(&calls),
            delete_during_poll: during_poll.then(|| (commands.clone(), agent)),
        });
        worker
            .state
            .set_draft_text(room, "must be discarded before delivery")
            .unwrap();
        worker.state.set_draft_recipients(room, [agent]).unwrap();
        worker.save(worker.state.clone()).unwrap();
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (events, received) = mpsc::channel();
        commands.send((1, BusCommand::Submit(room))).unwrap();
        if !during_poll {
            commands.send((2, BusCommand::DeleteAgent(agent))).unwrap();
            commands.send((3, BusCommand::Shutdown)).unwrap();
        }
        worker.run(receiver, events, Arc::clone(&snapshots));
        let expected = if during_poll {
            vec!["agent.list", "pane.close_if_identity"]
        } else {
            vec!["pane.close_if_identity"]
        };
        assert_eq!(*calls.lock().unwrap(), expected);
        assert!(snapshots.lock().unwrap().state.agent(agent).is_none());
        let outcomes: Vec<_> = received
            .try_iter()
            .filter_map(|event| match event {
                BusEvent::CommandFinished { command_id, result } => Some((command_id, result)),
                _ => None,
            })
            .collect();
        assert_eq!(outcomes, vec![(1, Ok(())), (2, Ok(())), (3, Ok(()))]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn codex_first_room_prompt_does_not_wait_for_its_deferred_session_start() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    queue(&mut worker, room, agent, "first real prompt");
    worker.submit_ready().unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["agent.prompt_if_unbound"]);
    // An uncertain first submit remains owned; it must not bootstrap twice.
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    let recovered_calls = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeTransport {
        replies: VecDeque::new(),
        calls: recovered_calls.clone(),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    queue(&mut recovered, room, agent, "second must wait");
    recovered.submit_ready().unwrap();
    assert!(recovered_calls.lock().unwrap().is_empty());
    assert!(recovered
        .state
        .agent(agent)
        .unwrap()
        .current_request
        .is_some());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

fn record(dir: &std::path::Path, provider: Provider, mut value: serde_json::Value) {
    if provider == Provider::Codex {
        // Root interactive Codex callbacks have a transcript; explicit null
        // fixtures model its title/memory background sessions instead.
        value
            .as_object_mut()
            .unwrap()
            .entry("transcript_path")
            .or_insert(json!("/tmp/bus-root-transcript.jsonl"));
    }
    callbacks::append(&dir.join("callbacks/launch"), "launch", provider, value).unwrap();
}

#[test]
fn deferred_codex_start_and_final_survive_background_sessions_without_rebinding() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "first room prompt");
    worker.submit_ready().unwrap();
    for value in [
        json!({"hook_event_name":"SessionStart","session_id":"title-session","transcript_path":null}),
        json!({"hook_event_name":"SessionStart","session_id":"root-session"}),
        json!({"hook_event_name":"UserPromptSubmit","session_id":"root-session","turn_id":"root-turn","prompt":"first room prompt"}),
        json!({"hook_event_name":"Stop","session_id":"root-session","turn_id":"root-turn","last_assistant_message":"ROOT_FINAL"}),
        json!({"hook_event_name":"SessionStart","session_id":"memory-session","transcript_path":null}),
        json!({"hook_event_name":"Stop","session_id":"title-session","turn_id":"title-turn","last_assistant_message":"wrong title","transcript_path":null}),
    ] {
        record(&dir, Provider::Codex, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "ROOT_FINAL"
    );
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("root-session")
    );
    assert!(
        !worker
            .state
            .agent(agent)
            .unwrap()
            .session_binding_invalidated
    );
    worker.submit_ready().unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["agent.prompt_if_unbound", "pane.report_agent_session"]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_claude_background_pending_and_trusted_continuation_update_work_settlement(
) {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = queue(&mut worker, room, agent, "review the plan");
    worker.submit_ready().unwrap();
    for value in [
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":"prompt-1","prompt":"review the plan"}),
        json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"prompt-1","last_assistant_message":"Waiting for reviewers","background_tasks":[{"task_id":"reviewer-1","status":"running"}],"session_crons":[]}),
    ] {
        record(&dir, Provider::ClaudeCode, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 20)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    let progress = worker.state.work_settlement(request).unwrap();
    assert_eq!(
        progress.provider_request_settlement,
        crate::bus::orchestrator::ProviderRequestSettlement::Active
    );
    assert_eq!(
        progress.callback_lineage.provider_turn.as_deref(),
        Some("prompt-1")
    );
    assert!(!progress.has_final_reply);
    let before_final_revision = worker.state.orchestrator_state().wake_revision(room);

    for value in [
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":"prompt-2","prompt":"<task-notification>reviewer-1 finished</task-notification>"}),
        json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"prompt-2","last_assistant_message":"Ready","background_tasks":[],"session_crons":[]}),
    ] {
        record(&dir, Provider::ClaudeCode, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();

    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker
            .state
            .request(request)
            .unwrap()
            .provider_turn_id
            .as_deref(),
        Some("prompt-2")
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "Ready"
    );
    let complete = worker.state.work_settlement(request).unwrap();
    assert_eq!(
        complete.provider_request_settlement,
        crate::bus::orchestrator::ProviderRequestSettlement::Completed
    );
    assert_eq!(
        complete.callback_lineage.provider_turn.as_deref(),
        Some("prompt-2")
    );
    assert!(complete.has_final_reply);
    let after_final_revision = worker.state.orchestrator_state().wake_revision(room);
    assert!(
        after_final_revision > before_final_revision,
        "the durable provider Request settlement must wake the room orchestrator"
    );
    assert!(worker
        .state
        .orchestrator_state()
        .context(room)
        .facts
        .iter()
        .any(|fact| matches!(
            fact,
            crate::bus::orchestrator::JournalFact::RequestSettled {
                request_id,
                settlement: crate::bus::orchestrator::ProviderRequestSettlement::Completed,
                ..
            } if *request_id == request
        )));
    let reloaded = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.orchestrator_state().wake_revision(room),
        after_final_revision,
        "settlement must remain queryable after the waiting CLI process exits or restarts"
    );
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn session_start_reports_bind_through_native_provider_session_adapter() {
    struct NativeSessionTransport(Arc<Mutex<Option<crate::agent_resume::AgentSessionRef>>>);
    impl Transport for NativeSessionTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if let Method::PaneReportAgentSession(params) = method {
                // The real API returns Ok even if this consumer rejects the source.
                // A transport-only mock would miss a silently discarded identity.
                *self.0.lock().unwrap() = crate::agent_resume::session_ref_from_report(
                    &params.source,
                    &params.agent,
                    params.agent_session_id,
                    params.agent_session_path,
                );
            }
            Ok(ResponseResult::Ok {})
        }
    }
    for provider in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor] {
        let (mut worker, agent, _, dir, _) = fixture(provider, vec![]);
        let native_session = Arc::new(Mutex::new(None));
        worker.transport = Box::new(NativeSessionTransport(native_session.clone()));
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        let callback = if provider == Provider::Cursor {
            json!({"hook_event_name":"sessionStart","conversation_id":"native-session"})
        } else {
            json!({"hook_event_name":"SessionStart","session_id":"native-session"})
        };
        record(&dir, provider, callback);
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
        assert_eq!(
            native_session
                .lock()
                .unwrap()
                .as_ref()
                .map(|session| session.value.as_str()),
            Some("native-session"),
            "native session adapter must retain {provider:?} identity"
        );
        assert_eq!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .runtime_identity
                .session_id
                .as_deref(),
            Some("native-session")
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn failed_draft_command_keeps_its_outcome_after_later_success() {
    // Removing command-specific results must fail: an aggregate acknowledgement
    // cannot tell the shell whether it may discard a pending local draft edit.
    let (worker, _, room, dir, _) = fixture(Provider::Codex, vec![]);
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (commands, receiver) = mpsc::sync_channel(8);
    let (events, outcomes) = mpsc::channel();
    commands
        .send((
            11,
            BusCommand::SetDraftText(RoomId(u64::MAX), "unsaved".into()),
        ))
        .unwrap();
    commands
        .send((12, BusCommand::SetNotes(room, "saved notes".into())))
        .unwrap();
    drop(commands);
    worker.run(receiver, events, Arc::clone(&snapshots));
    let results: Vec<_> = outcomes
        .try_iter()
        .filter_map(|event| match event {
            BusEvent::CommandFinished { command_id, result } => Some((command_id, result)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results.len(),
        2,
        "each processed command needs its own outcome"
    );
    assert_eq!(results[0].0, 11);
    assert!(results[0].1.is_err());
    assert_eq!(results[1], (12, Ok(())));
    let snapshot = snapshots.lock().unwrap();
    assert_eq!(snapshot.last_command_id, 12);
    assert_eq!(snapshot.state.room(room).unwrap().notes, "saved notes");
    drop(snapshot);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn initial_recovery_tick_keeps_detached_room_reply_unread_until_selected() {
    // A saved visible room belongs to the detached client. The first coordinator
    // tick must not mark its newly spooled reply seen before any UI selection.
    let (mut worker, agent, room_b, dir, _) = fixture(Provider::Codex, vec![]);
    let room_a = worker.state.create_room("other room").unwrap();
    worker.state.select_room(room_b).unwrap();
    let request = queue(&mut worker, room_b, agent, "finish while detached");
    worker.state.begin_submission(request, "launch", 0).unwrap();
    worker
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"finish while detached"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    assert_eq!(worker.state.room(room_b).unwrap().unread_count, 0);
    drop(worker);
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"completed while detached"}),
    );
    let info = serde_json::from_value(json!({
        "terminal_id": "terminal", "agent": "codex", "agent_status": "idle",
        "agent_session": {"source": "herdr:codex", "agent": "codex", "kind": "id", "value": "session"},
        "workspace_id": "workspace", "tab_id": "tab", "pane_id": "pane",
        "focused": false, "interactive_ready": true, "revision": 1
    }))
    .unwrap();
    let fake = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
        calls: Arc::new(Mutex::new(Vec::new())),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered.tick().unwrap();
    assert_eq!(
        recovered.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        recovered.state.room(room_b).unwrap().latest_replies[&agent].text,
        "completed while detached"
    );
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 1);
    let (events, _) = mpsc::channel();
    recovered
        .command(BusCommand::SelectRoom(room_a), &events)
        .unwrap();
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 1);
    assert_eq!(
        recovered
            .store
            .load()
            .unwrap()
            .unwrap()
            .room(room_b)
            .unwrap()
            .unread_count,
        1
    );
    recovered
        .command(BusCommand::SelectRoom(room_b), &events)
        .unwrap();
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 0);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncertain_submission_survives_restart_and_never_retries() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "lost response".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "same");
    let next = queue(&mut worker, room, agent, "next");
    worker.submit_ready().unwrap();
    assert!(worker.state.request(request).unwrap().uncertain_outcome);
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    let (transport_calls, no_replies) = (Arc::new(Mutex::new(Vec::new())), VecDeque::new());
    let fake = FakeTransport {
        replies: no_replies,
        calls: transport_calls.clone(),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .state
        .observe_status(agent, RuntimeStatus::Idle, 3)
        .unwrap();
    recovered.submit_ready().unwrap();
    assert!(transport_calls.lock().unwrap().is_empty());
    assert_eq!(recovered.state.queued_requests(agent), &[next]);
    assert_eq!(
        recovered.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncertain_submission_rejects_wrong_session_start_binding() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "lost response".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "same");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"wrong-session","turn_id":"wrong-turn","prompt":"same"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"wrong-session","turn_id":"wrong-turn","last_assistant_message":"wrong reply"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(!worker.state.request(request).unwrap().trusted_start_bound);
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn changed_session_start_cannot_erase_and_rebind_known_identity() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "held after session changed");
    for _ in 0..2 {
        record(
            &dir,
            Provider::Codex,
            json!({"hook_event_name":"SessionStart","session_id":"other-session"}),
        );
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("session")
    );
    assert!(calls.lock().unwrap().is_empty());
    // A stale server identity/Idle observation must not reopen this launch.
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert!(worker.state.confirm_hook_setup(agent).is_err());
    worker.save(worker.state.clone()).unwrap();
    let mut reloaded = worker.store.load().unwrap().unwrap();
    assert!(reloaded.agent(agent).unwrap().session_binding_invalidated);
    assert!(reloaded.confirm_hook_setup(agent).is_err());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn busy_rejection_restores_exact_fifo_and_blocked_does_not_submit() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "busy".into(),
            code: Some("agent_not_idle".into()),
            definitely_rejected: true,
        })],
    );
    let first = queue(&mut worker, room, agent, "first");
    let second = queue(&mut worker, room, agent, "second");
    worker.submit_ready().unwrap();
    assert_eq!(worker.state.queued_requests(agent), &[first, second]);
    assert!(worker.state.agent(agent).unwrap().current_request.is_none());
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 3)
        .unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cursor_stop_before_response_and_start_survives_detach_without_duplicate_reply() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Cursor,
        vec![Err(TransportError {
            message: "unknown".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "same");
    worker.submit_ready().unwrap();
    let transcript = dir.join("cursor-session.jsonl");
    let completed_transcript = concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"same\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I will ask first.\"},{\"type\":\"tool_use\",\"name\":\"AskQuestion\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"real final\"}]}}\n",
            "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
    );
    // Cursor can publish hooks before its transcript writer finishes the turn.
    std::fs::write(
        &transcript,
        completed_transcript
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn","status":"completed"}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn","text":"I will ask first.real final","transcript_path":transcript}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .is_some_and(|message| message.contains("completed transcript")));
    assert_eq!(
        callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .len(),
        2
    );
    std::fs::write(&transcript, completed_transcript).unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn","prompt":"same"}),
    );
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 4)
        .unwrap();
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    worker
        .state
        .observe_status(agent, RuntimeStatus::Working, 5)
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 6)
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 7)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "real final"
    );
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    // Replayed raw callbacks cannot increment unread again after a crash/delete boundary.
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn","status":"completed"}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn","text":"I will ask first.real final","transcript_path":transcript}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    // A second request may legitimately produce exactly the same answer. Its
    // callback identity, not the uniqueness of its text, owns the room reply.
    let next = queue(&mut worker, room, agent, "same");
    worker.submit_ready().unwrap();
    std::fs::write(
        &transcript,
        format!("{completed_transcript}{completed_transcript}"),
    )
    .unwrap();
    for value in [
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-2","prompt":"same"}),
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-2","status":"completed"}),
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn-2","text":"I will ask first.real final","transcript_path":transcript}),
    ] {
        record(&dir, Provider::Cursor, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 8)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    for (id, turn) in [(request, "turn"), (next, "turn-2")] {
        let original = worker.state.request(id).unwrap();
        assert_eq!(original.phase, RequestPhase::Completed);
        assert_eq!(original.provider_turn_id.as_deref(), Some(turn));
        assert_eq!(original.pending_final.as_ref().unwrap().text, "real final");
    }
    assert_eq!(worker.state.room(room).unwrap().unread_count, 2);
    assert_eq!(worker.state.agent(agent).unwrap().current_request, None);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn storage_failure_prevents_send_and_startup_failure_releases_coordinator() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "prompt");
    io::atomic_write(&dir.join("state.json"), b"corrupt").unwrap();
    assert!(worker.submit_ready().is_err());
    assert!(calls.lock().unwrap().is_empty());
    assert!(worker.storage_failed);
    drop(worker);
    let fake = || {
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: calls.clone(),
            state_path: dir.join("state.json"),
        })
    };
    assert!(Worker::open(dir.clone(), fake()).is_err());
    assert!(io::lock(&dir.join("coordinator.lock")).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn late_old_identical_codex_final_cannot_bind_and_provider_error_holds_queue() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "unknown".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"old","prompt":"same"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let request = queue(&mut worker, room, agent, "same");
    let next = queue(&mut worker, room, agent, "next");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"old","last_assistant_message":"stale same prompt final"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"current","prompt":"same"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"StopFailure","session_id":"session","turn_id":"current"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    assert_eq!(worker.state.queued_requests(agent), &[next]);
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_ref()
        .unwrap()
        .contains("failed"));
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn claude_still_requires_real_session_start_before_submission() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::ClaudeCode, vec![]);
    queue(&mut worker, room, agent, "queued");
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart","session_id":"fresh-session"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("fresh-session")
    );
    assert_eq!(*calls.lock().unwrap(), vec!["pane.report_agent_session"]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
