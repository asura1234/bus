use super::*;

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
    assert_eq!(calls[0]["text"], "only once after resume");
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
fn upgraded_agents_receive_current_messaging_rules_once_before_waiting_work() {
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
                pending.len(),
                2,
                "missing upgrade onboarding: {provider:?}/{role}"
            );
            assert_eq!(pending[1], queued, "onboarding must precede waiting work");
            let onboarding = resumed.state.request(pending[0]).unwrap();
            assert!(onboarding.queue_only);
            assert_eq!(onboarding.room_id, home);
            assert_eq!(onboarding.prompt.author, Author::Bus);
            assert!(onboarding
                .prompt
                .text
                .contains("Bus automatically adds 'name (agent id, room R):'"));
            assert!(onboarding
                .prompt
                .text
                .contains("do not add a 'from X', 'X here' or signature line"));
            assert!(onboarding
                .prompt
                .text
                .contains("Messages from another room wait for the next turn"));
            assert!(std::fs::read_to_string(dir.join("docs/how-to-bus-cli.md"))
                .unwrap()
                .contains("conversation room"));
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
