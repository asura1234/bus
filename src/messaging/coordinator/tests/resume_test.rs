use super::support::{test_prompt_content, test_request_payload};
use super::*;

#[test]
fn global_names_resume_preserves_legacy_sender_identity_without_a_snapshot() {
    assert_global_names_legacy_sender(None, None);
}

#[test]
fn global_names_resume_keeps_existing_sender_snapshot() {
    assert_global_names_legacy_sender(Some("captured-author"), None);
}

#[test]
fn global_names_resume_keeps_submitted_payload() {
    assert_global_names_legacy_sender(None, Some("already submitted payload"));
}

fn assert_global_names_legacy_sender(snapshot: Option<&str>, submitted: Option<&str>) {
    use crate::messaging::model::{Author, Draft};

    let (mut worker, first, _, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    let room = state.create_room("legacy").unwrap();
    let author = state
        .create_agent(room, "second-author", Provider::Codex, dir.clone(), None)
        .unwrap();
    let recipient = state
        .create_agent(room, "recipient", Provider::Codex, dir.clone(), None)
        .unwrap();
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "legacy question".into(),
                files: vec![],
                recipient_ids: [recipient].into(),
            },
            Author::Agent(author),
            io::now_ms(),
        )
        .unwrap()[0];
    let mut document = serde_json::to_value(&state).unwrap();
    document["agents"][author.0.to_string()]["name"] = json!("author");
    if let Some(name) = snapshot {
        document["requests"][request.0.to_string()]["sender_identity"] = json!([name, room]);
    } else {
        document["requests"][request.0.to_string()]
            .as_object_mut()
            .unwrap()
            .remove("sender_identity");
    }
    document["requests"][request.0.to_string()]["submitted_payload"] = json!(submitted);
    worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    drop(worker);
    let mut worker = Worker::open(
        dir.clone(),
        Box::new(FakeTransport {
            replies: vec![].into(),
            calls,
            state_path: dir.join("state.json"),
        }),
    )
    .unwrap();
    assert_eq!(worker.state.agent(first).unwrap().name, "author");
    assert_eq!(
        worker.state.agent(author).unwrap().name,
        format!("author-{}", author.0)
    );
    assert_eq!(
        worker.state.request(request).unwrap().prompt.author,
        Author::Agent(author)
    );
    assert!(worker.state.queued_requests(recipient).contains(&request));
    assert_eq!(
        worker.state.request(request).unwrap().sender_identity,
        Some((snapshot.unwrap_or("author").into(), room))
    );
    // Current names are unique, but the old queued author's name may now
    // belong to the survivor. Preserve that historical name with its real ID.
    // Same-room payloads use the compact header; frozen payloads stay exact.
    let expected = submitted
        .map(str::to_owned)
        .unwrap_or_else(|| match snapshot {
            Some(name) => format!("{name} (agent {}):\nlegacy question", author.0),
            None => format!("author (agent {}):\nlegacy question", author.0),
        });
    let actual = worker.state.render_request_payload(request).unwrap();
    let actual = if submitted.is_some() {
        actual
    } else {
        test_request_payload(&mut worker.state, request).unwrap()
    };
    assert_eq!(
        actual, expected,
        "migration must preserve the historical sender name of a legacy queued message"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn global_names_resume_repairs_duplicates_once_preserving_ids_binding_and_queue() {
    let (mut worker, first, _, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    let side = state.create_room("side").unwrap();
    let second = state
        .create_agent(side, "second", Provider::Codex, dir.clone(), None)
        .unwrap();
    let reserved = format!("author-{}", second.0);
    let third = state
        .create_agent(side, &reserved, Provider::Codex, dir.clone(), None)
        .unwrap();
    let master = state.master_room().unwrap().id;
    let orchestrator = state
        .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(orchestrator, side).unwrap();
    worker.save(state).unwrap();
    let request = queue(&mut worker, side, second, "retained work");
    let original_request = worker.state.request(request).unwrap().clone();
    let identity = worker.state.agent(first).unwrap().runtime_identity.clone();
    let mut document = serde_json::to_value(&worker.state).unwrap();
    for id in [second, orchestrator] {
        document["agents"][id.0.to_string()]["name"] = json!("author");
    }
    worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    drop(worker);
    let open = || {
        Worker::open(
            dir.clone(),
            Box::new(FakeTransport {
                replies: vec![].into(),
                calls: Arc::clone(&calls),
                state_path: dir.join("state.json"),
            }),
        )
        .unwrap()
    };
    let worker = open();
    assert_eq!(worker.state.agent(first).unwrap().name, "author");
    assert_eq!(
        worker.state.agent(second).unwrap().name,
        format!("{reserved}-2")
    );
    assert_eq!(worker.state.agent(third).unwrap().name, reserved);
    assert_eq!(
        worker.state.agent(orchestrator).unwrap().name,
        format!("author-{}", orchestrator.0)
    );
    assert_eq!(
        worker.state.agent(first).unwrap().runtime_identity,
        identity
    );
    assert_eq!(worker.state.orchestrator_of(side).unwrap().id, orchestrator);
    assert_eq!(worker.state.request(request).unwrap(), &original_request);
    assert!(worker.state.queued_requests(second).contains(&request));
    let notices: Vec<_> = worker
        .state
        .notices(side)
        .filter(|p| p.text.contains("Agent renamed"))
        .map(|p| p.text.clone())
        .collect();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains(&format!("{reserved}-2")));
    drop(worker);
    let worker = open();
    assert_eq!(
        worker.state.agent(second).unwrap().name,
        format!("{reserved}-2")
    );
    assert_eq!(
        worker
            .state
            .notices(side)
            .filter(|p| p.text.contains("Agent renamed"))
            .count(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reply_capture_diagnostics_record_loaded_ownership_before_restart_observations() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "PRIVATE_PROMPT");
    worker.submit_ready().unwrap();
    let before = worker.state.request(request).unwrap().clone();
    let queued = queue(&mut worker, room, agent, "PRIVATE_QUEUED");
    let transport = FakeTransport {
        replies: vec![].into(),
        calls,
        state_path: dir.join("state.json"),
    };
    drop(worker);
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let worker = Worker::open(dir.clone(), Box::new(transport)).unwrap();
        assert_eq!(worker.state.request(request).unwrap(), &before);
        let log = capture.text();
        let row = log
            .lines()
            .find(|line| line.contains("bus.resume.open_request"))
            .expect("startup must record the saved request before replacing status observations");
        assert!(row.contains(&format!("request_id={} ", request.0)), "{row}");
        assert!(row.contains("uncertain_outcome=true"), "{row}");
        assert!(
            !log.lines()
                .any(|line| line.contains("bus.resume.open_request")
                    && line.contains(&format!("request_id={} ", queued.0))),
            "{log}"
        );
        assert!(!log.contains("PRIVATE"), "{log}");
    });
    std::fs::remove_dir_all(dir).unwrap();
}

fn resumed_info(provider: Provider) -> schema::AgentInfo {
    let kind = crate::messaging::identity::provider_kind(provider).label();
    serde_json::from_value(json!({
        "terminal_id": "restored-terminal", "pane_id": "pane",
        "name": "bus-r1-a2", "agent": kind, "agent_status": "idle",
        "agent_session": {"source": crate::messaging::identity::managed_source(provider), "agent": kind, "kind": "id", "value": "session"},
        "workspace_id": "workspace", "tab_id": "tab", "focused": false,
        "interactive_ready": true, "revision": 1
    })).unwrap()
}

/// Native fault injection: the resumed CLI prints its rejection and exits,
/// leaving the pane's shell alive. No real provider or live Bus is contacted.
struct RejectedResume;

impl Transport for RejectedResume {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![serde_json::from_value(json!({
                "terminal_id":"terminal", "pane_id":"pane", "workspace_id":"w1", "tab_id":"t1", "focused":false, "revision":1, "agent_status":"unknown",
                "native_resume":{"name":"bus-r1-a2","agent":"claude","session_id":"session","command":"claude --resume session","error":"provider exited before startup confirmation"}
            })).unwrap()] }),
            Method::PaneGet(_) => Ok(ResponseResult::PaneInfo {
                pane: serde_json::from_value(json!({
                    "pane_id":"pane", "terminal_id":"terminal", "workspace_id":"w1",
                    "tab_id":"t1", "focused":false, "agent_status":"unknown", "revision":1,
                    "agent_process_exited":true
                }))
                .unwrap(),
            }),
            Method::PaneRead(_) => Ok(ResponseResult::PaneRead {
                read: serde_json::from_value(json!({
                    "pane_id":"pane", "workspace_id":"w1", "tab_id":"t1", "revision":1,
                    "source":"recent", "format":"text", "truncated":false,
                    "text":"No conversation found with session ID: session\nscratch% "
                }))
                .unwrap(),
            }),
            other => panic!("failed resume must preserve the shell and queued work: {other:?}"),
        }
    }
}

#[test]
fn restart_resume_rejection_surfaces_actionable_error_and_preserves_work() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = queue(&mut worker, room, agent, "retained task");
    let identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(RejectedResume)).unwrap();
    let infos = worker.poll().unwrap().unwrap();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
        .unwrap();
    let failed = worker.state.agent(agent).unwrap();
    let error = failed
        .actionable_error
        .as_deref()
        .expect("restart must monitor resume startup");
    assert!(
        error.contains("No conversation found with session ID: session"),
        "{error}"
    );
    assert!(error.contains("resume"), "{error}");
    assert_eq!(failed.status, RuntimeStatus::Unavailable);
    assert_eq!(failed.runtime_identity, identity);
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert!(failed.startup.as_ref().unwrap().reported);
    drop(worker);
    let worker = Worker::open(dir.clone(), Box::new(RejectedResume)).unwrap();
    assert!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .startup
            .as_ref()
            .unwrap()
            .reported
    );
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .is_some());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn missing_pane_after_restart_is_cleared_but_transport_failure_is_not_evidence() {
    struct MissingPane {
        code: &'static str,
        rejected: bool,
    }
    impl Transport for MissingPane {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![] }),
                Method::PaneGet(_) => Err(TransportError {
                    code: Some(self.code.into()),
                    message: self.code.into(),
                    definitely_rejected: self.rejected,
                }),
                other => panic!("unexpected operation: {other:?}"),
            }
        }
    }
    // The transport's submission rejection bit is false for PaneGet's
    // pane_not_found response; it describes PTY input, not read evidence.
    for (code, rejected, cleared) in [
        ("pane_not_found", false, true),
        ("pane_not_found", true, true),
        ("timeout", false, false),
    ] {
        let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
        worker.transport = Box::new(MissingPane { code, rejected });
        worker.poll().unwrap();
        let observed = worker.state.agent(agent).unwrap();
        assert_eq!(
            observed.runtime_identity.pane_id.is_none(),
            cleared,
            "{code}"
        );
        assert_eq!(
            observed.runtime_identity.terminal_id.as_deref(),
            Some("terminal")
        );
        assert_eq!(
            observed.runtime_identity.session_id.as_deref(),
            Some("session")
        );
        if cleared {
            assert!(observed.actionable_error.is_some());
        }
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn cold_resume_rebinds_same_conversation_and_persists_new_terminal_for_each_provider() {
    // Bus restores the same provider conversation in a newly allocated PTY.
    // Requiring the old PTY ID incorrectly makes the restored agent unavailable.
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let (mut worker, agent, _room, dir, _calls) = fixture(
            provider,
            vec![Ok(ResponseResult::AgentList {
                agents: vec![resumed_info(provider)],
            })],
        );
        worker.poll().unwrap();
        let restored = worker.state.agent(agent).unwrap();
        assert_eq!(restored.status, RuntimeStatus::Idle, "{provider:?}");
        assert_eq!(
            restored.runtime_identity.terminal_id.as_deref(),
            Some("restored-terminal")
        );
        assert_eq!(
            restored.runtime_identity.session_id.as_deref(),
            Some("session")
        );
        assert_eq!(
            restored.runtime_identity.launch_id.as_deref(),
            Some("launch")
        );
        assert!(restored.hook_setup_confirmed);
        let saved = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.agent(agent).unwrap().runtime_identity,
            restored.runtime_identity
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn cold_resume_rebinds_unstarted_codex_and_delivers_its_first_prompt_once() {
    let mut info = resumed_info(Provider::Codex);
    info.agent_session = None;
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![
            Ok(ResponseResult::AgentList {
                agents: vec![info.clone()],
            }),
            Ok(ResponseResult::AgentInfo { agent: info }),
            Ok(ResponseResult::Ok {}),
        ],
    );
    let mut state = worker.state.clone();
    let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    state.set_agent_runtime_identity(agent, identity).unwrap();
    worker.save(state).unwrap();
    let request = queue(&mut worker, room, agent, "first prompt after restart");
    worker.poll().unwrap();
    let restored = worker.state.agent(agent).unwrap();
    assert_eq!(restored.status, RuntimeStatus::Idle);
    assert_eq!(
        restored.runtime_identity.terminal_id.as_deref(),
        Some("restored-terminal")
    );
    assert!(restored.runtime_identity.session_id.is_none());
    worker.submit_ready().unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|method| **method == "agent.prompt_if_unbound")
            .count(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cold_resume_rejects_foreign_missing_and_ambiguous_ownership() {
    for case in [
        "name",
        "provider",
        "session",
        "source",
        "kind",
        "missing-session",
        "ambiguous",
        "other-owner",
        "invalidated",
        "unbound",
    ] {
        let mut info = resumed_info(Provider::Codex);
        match case {
            "name" => info.name = Some("other-agent".into()),
            "provider" => info.agent = Some("claude".into()),
            "session" => info.agent_session.as_mut().unwrap().value = "foreign".into(),
            "source" => info.agent_session.as_mut().unwrap().source = "untrusted".into(),
            "kind" => info.agent_session.as_mut().unwrap().kind = schema::AgentSessionRefKind::Path,
            "missing-session" => info.agent_session = None,
            _ => {}
        }
        let mut infos = vec![info];
        if case == "ambiguous" {
            let mut other = infos[0].clone();
            other.terminal_id = "duplicate".into();
            other.pane_id = "duplicate-pane".into();
            infos.push(other);
        }
        let (mut worker, agent, room, dir, calls) = fixture(
            Provider::Codex,
            vec![Ok(ResponseResult::AgentList { agents: infos })],
        );
        let mut state = worker.state.clone();
        match case {
            "other-owner" => {
                let other = state
                    .create_agent(room, "other", Provider::Codex, dir.clone(), None)
                    .unwrap();
                state
                    .set_agent_runtime_identity(
                        other,
                        AgentRuntimeIdentity {
                            terminal_id: Some("restored-terminal".into()),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            "invalidated" => state.invalidate_agent_session(agent).unwrap(),
            "unbound" => {
                let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
                identity.session_id = None;
                state.set_agent_runtime_identity(agent, identity).unwrap();
            }
            _ => {}
        }
        worker.save(state).unwrap();
        queue(&mut worker, room, agent, "must not deliver");
        worker.poll().unwrap();
        worker.submit_ready().unwrap();
        assert_eq!(
            worker.state.agent(agent).unwrap().status,
            RuntimeStatus::Unavailable,
            "{case}"
        );
        assert_eq!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .runtime_identity
                .terminal_id
                .as_deref(),
            Some("terminal"),
            "{case}"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["agent.list", "pane.get"],
            "{case}"
        );
        assert_eq!(
            worker
                .state
                .notices(worker.state.master_room().unwrap().id)
                .count(),
            0,
            "unknown exit evidence must not notify: {case}"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn cold_resume_preserves_uncertain_request_and_settles_its_original_reply_once() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "original instruction");
    worker.submit_ready().unwrap(); // Its transport outcome is unknown: never send twice.
    let queued = queue(&mut worker, room, agent, "next instruction");
    let before = worker.state.request(request).unwrap().clone();
    let transport = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList {
            agents: vec![resumed_info(Provider::Codex)],
        })]
        .into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(transport)).unwrap();
    worker.poll().unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(worker.state.request(request).unwrap(), &before);
    assert_eq!(
        worker.state.request(queued).unwrap().phase,
        RequestPhase::Queued
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|m| **m == "agent.prompt_if_idle")
            .count(),
        1
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"original-turn","prompt":"original instruction"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"original-turn","last_assistant_message":"original reply after restart"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    // Repeated callbacks/polls cannot attach this final to the next request.
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].request_id,
        request
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "original reply after restart"
    );
    assert_eq!(
        worker.state.request(queued).unwrap().phase,
        RequestPhase::Queued
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn interactive_ready_releases_setup_gate_but_pending_deletion_stays_closed() {
    for deleting in [false, true] {
        let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
        let request = queue(&mut worker, room, agent, "original");
        worker.submit_ready().unwrap();
        let mut state = worker.state.clone();
        state
            .observe_status(agent, RuntimeStatus::Working, 3)
            .unwrap();
        worker.save(state).unwrap();
        record(
            &dir,
            Provider::Codex,
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"original"}),
        );
        record(
            &dir,
            Provider::Codex,
            json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"pending final"}),
        );
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
        let mut value = serde_json::to_value(&worker.state).unwrap();
        value["agents"][agent.0.to_string()]["hook_setup_confirmed"] = json!(false);
        value["agents"][agent.0.to_string()]["deletion_pending"] = json!(deleting);
        value["agents"][agent.0.to_string()]["actionable_error"] =
            json!("keep setup or deletion warning");
        worker.save(serde_json::from_value(value).unwrap()).unwrap();
        worker.transport = Box::new(FakeTransport {
            replies: vec![Ok(ResponseResult::AgentList {
                agents: vec![resumed_info(Provider::Codex)],
            })]
            .into(),
            calls: Arc::clone(&calls),
            state_path: dir.join("state.json"),
        });
        worker.poll().unwrap();
        let restored = worker.state.agent(agent).unwrap();
        assert_eq!(restored.hook_setup_confirmed, !deleting);
        assert_eq!(restored.deletion_pending, deleting);
        assert_eq!(
            restored.runtime_identity.terminal_id.as_deref(),
            Some("restored-terminal")
        );
        if deleting {
            assert_eq!(restored.status, RuntimeStatus::Unavailable);
            assert_eq!(
                restored.actionable_error.as_deref(),
                Some("keep setup or deletion warning")
            );
            assert_ne!(
                worker.state.request(request).unwrap().phase,
                RequestPhase::Completed
            );
            assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
        } else {
            assert_eq!(restored.status, RuntimeStatus::Idle);
            assert!(restored.actionable_error.is_none());
        }
        worker.submit_ready().unwrap();
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|m| **m == "agent.prompt_if_idle")
                .count(),
            1
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

struct ResumeDeliveryTransport {
    info: schema::AgentInfo,
    submitted: Arc<Mutex<Vec<serde_json::Value>>>,
    state_path: PathBuf,
}

impl Transport for ResumeDeliveryTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList {
                agents: vec![self.info.clone()],
            }),
            Method::AgentPromptIfIdle(params) => {
                let saved = JsonStore::new(self.state_path.clone())
                    .load()
                    .unwrap()
                    .unwrap();
                let agent = saved.agents().next().unwrap();
                assert_eq!(
                    agent.runtime_identity.terminal_id.as_deref(),
                    Some(params.expected_terminal_id.as_str())
                );
                assert_eq!(
                    saved.requests().next().unwrap().phase,
                    RequestPhase::Submitting
                );
                self.submitted
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(params).unwrap());
                // Unknown acknowledgement: preserve uncertainty instead of retrying.
                Ok(ResponseResult::Ok {})
            }
            _ => panic!("unexpected restore transport call: {method:?}"),
        }
    }
}

#[test]
fn cold_resume_delivers_queued_prompt_once_using_the_durably_rebound_identity() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "only once after resume");
    let submitted = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(ResumeDeliveryTransport {
        info: resumed_info(Provider::Codex),
        submitted: Arc::clone(&submitted),
        state_path: dir.join("state.json"),
    });
    worker.tick().unwrap();
    worker.tick().unwrap();
    let calls = submitted.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["target"], "pane");
    assert_eq!(calls[0]["expected_terminal_id"], "restored-terminal");
    assert_eq!(calls[0]["expected_session_id"], "session");
    assert_eq!(
        test_prompt_content(calls[0]["text"].as_str().unwrap()),
        "only once after resume"
    );
    assert!(worker.state.request(request).unwrap().uncertain_outcome);
    drop(calls);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cold_resume_save_failure_cannot_deliver_or_adopt_an_unpersisted_identity() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Ok(ResponseResult::AgentList {
            agents: vec![resumed_info(Provider::Codex)],
        })],
    );
    let request = queue(&mut worker, room, agent, "must wait for durable identity");
    std::fs::rename(dir.join("state.json"), dir.join("saved-state.json")).unwrap();
    std::fs::create_dir(dir.join("state.json")).unwrap();
    assert!(worker.tick().is_err());
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .terminal_id
            .as_deref(),
        Some("terminal")
    );
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert_eq!(*calls.lock().unwrap(), vec!["agent.list"]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn upgraded_agents_keep_saved_instructions_and_pending_work_without_new_turns() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        for role in ["worker", "default-orchestrator", "custom-orchestrator"] {
            let (mut worker, original, room, dir, calls) = fixture(provider, vec![]);
            let agent = if role == "worker" {
                original
            } else {
                let master = worker.state.master_room().unwrap().id;
                let id = worker
                    .state
                    .create_agent(master, role, provider, dir.clone(), None)
                    .unwrap();
                worker.state.bind_orchestrator(id, room).unwrap();
                worker
                    .state
                    .set_agent_runtime_identity(
                        id,
                        AgentRuntimeIdentity {
                            launch_id: Some("legacy".into()),
                            terminal_id: Some("orchestrator-terminal".into()),
                            pane_id: Some("orchestrator-pane".into()),
                            session_id: Some("orchestrator-session".into()),
                        },
                    )
                    .unwrap();
                id
            };
            let home = worker.state.agent(agent).unwrap().room_id;
            let active = if role == "worker" {
                let id = queue(&mut worker, home, agent, "already in flight");
                worker.state.begin_submission(id, "launch", 0).unwrap();
                Some((id, worker.state.request(id).unwrap().clone()))
            } else {
                None
            };
            let queued = queue(&mut worker, home, agent, "work waiting before upgrade");
            let launch = worker
                .state
                .agent(agent)
                .unwrap()
                .runtime_identity
                .launch_id
                .as_ref()
                .unwrap();
            let spool = dir.join("callbacks").join(launch);
            std::fs::create_dir_all(&spool).unwrap();
            let prompt = spool.join("system-prompt.md");
            if role != "worker" {
                std::fs::write(&prompt, format!("old {role} instructions")).unwrap();
            }
            let old_prompt = std::fs::read(&prompt).ok();
            // A pre-upgrade state has no knowledge of the new messaging rules.
            let mut document = serde_json::to_value(&worker.state).unwrap();
            document["agents"][agent.0.to_string()]
                .as_object_mut()
                .unwrap()
                .remove("messaging_rules_version");
            let legacy = serde_json::from_value(document).unwrap();
            worker.store.save(&legacy).unwrap();
            drop(worker);
            let transport = || FakeTransport {
                replies: VecDeque::new(),
                calls: Arc::clone(&calls),
                state_path: dir.join("state.json"),
            };
            let resumed = Worker::open(dir.clone(), Box::new(transport())).unwrap();
            if let Some((id, before)) = &active {
                assert_eq!(
                    resumed.state.agent(agent).unwrap().current_request,
                    Some(*id)
                );
                assert_eq!(
                    resumed.state.request(*id).unwrap(),
                    before,
                    "upgrade must preserve active ownership"
                );
            }
            let pending = resumed.state.queued_requests(agent).to_vec();
            assert_eq!(
                pending,
                vec![queued],
                "upgrade must not prepend a rules-refresh task: {provider:?}/{role}"
            );
            assert_eq!(
                std::fs::read(&prompt).ok(),
                old_prompt,
                "custom/saved prompt must survive"
            );
            drop(resumed);
            let again = Worker::open(dir.clone(), Box::new(transport())).unwrap();
            assert_eq!(
                again.state.queued_requests(agent),
                pending,
                "restart must not enqueue twice"
            );
            drop(again);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
