use super::*;

fn resumed_info(provider: Provider) -> schema::AgentInfo {
    let kind = launch::provider_kind(provider);
    serde_json::from_value(json!({
        "terminal_id": "restored-terminal", "pane_id": "pane",
        "name": "bus-r1-a2", "agent": kind, "agent_status": "idle",
        "agent_session": {"source": format!("herdr:{kind}"), "agent": kind, "kind": "id", "value": "session"},
        "workspace_id": "workspace", "tab_id": "tab", "focused": false,
        "interactive_ready": true, "revision": 1
    })).unwrap()
}

#[test]
fn cold_resume_rebinds_same_conversation_and_persists_new_terminal_for_each_provider() {
    // Herdr restores the same provider conversation in a newly allocated PTY.
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
            "kind" => {
                info.agent_session.as_mut().unwrap().kind =
                    crate::agent_resume::AgentSessionRefKind::Path
            }
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
        assert_eq!(*calls.lock().unwrap(), vec!["agent.list"], "{case}");
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
fn cold_resume_does_not_grant_hook_consent_or_settle_a_pending_deletion() {
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
        assert!(!restored.hook_setup_confirmed);
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
