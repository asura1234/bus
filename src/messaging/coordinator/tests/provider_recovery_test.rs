use super::*;

struct MissingNative;
impl Transport for MissingNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![] }),
            Method::PaneGet(p) => Ok(ResponseResult::PaneInfo { pane: serde_json::from_value(json!({
                "pane_id":p.pane_id, "terminal_id":"terminal", "workspace_id":"w1", "tab_id":"t1",
                "focused":false, "agent_status":"unknown", "revision":1
            })).unwrap() }),
            Method::PaneRead(p) => Ok(ResponseResult::PaneRead { read: serde_json::from_value(json!({
                "pane_id":p.pane_id, "workspace_id":"w1", "tab_id":"t1", "source":"recent", "format":"text",
                "revision":1, "text":"zsh: command not found: codex\nscratch% ", "truncated":false, "returned_lines":2
            })).unwrap() }),
            other => panic!("unexpected operation {other:?}"),
        }
    }
}

#[test]
fn codexup_missing_binary_blocks_the_room_once_and_retains_queued_work() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "recoverable task");
    let mut state = worker.state.clone();
    let second = state
        .create_agent(room, "second", Provider::Codex, dir.clone(), None)
        .unwrap();
    let identity = state.agent(agent).unwrap().runtime_identity.clone();
    state
        .set_agent_runtime_identity(
            second,
            AgentRuntimeIdentity {
                pane_id: Some("pane2".into()),
                ..identity
            },
        )
        .unwrap();
    state
        .observe_status(second, RuntimeStatus::Unavailable, 1)
        .unwrap();
    state.confirm_hook_setup(second).unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(MissingNative);
    worker.poll().unwrap();
    worker.poll().unwrap();
    worker
        .expire_queued_requests(io::now_ms() + 180_000)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert_eq!(worker.state.room_status(room), RuntimeStatus::Blocked);
    let notices: Vec<_> = worker
        .state
        .notices(room)
        .filter(|n| n.text.starts_with("BLOCKED") && n.text.contains("codex"))
        .collect();
    assert_eq!(
        notices.len(),
        1,
        "one room notice must replace per-agent failures"
    );
    assert!(worker.state.agent(agent).is_some());
    assert!(worker.state.agent(second).is_some());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

struct RecoveryNative {
    starts: Arc<Mutex<Vec<schema::AgentStartParams>>>,
    refuse_close: bool,
}
impl Transport for RecoveryNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::TabCreate(p) => {
                assert_eq!(
                    p.env.get("BUS_LAUNCH_ID").map(String::as_str),
                    Some("launch")
                );
                Ok(ResponseResult::TabCreated {
                    tab: serde_json::from_value(json!({"tab_id":"new", "workspace_id":"w1", "number":2, "label":"resumed", "focused":false, "pane_count":1, "agent_status":"unknown"})).unwrap(),
                    root_pane: serde_json::from_value(json!({"pane_id":"new-pane", "terminal_id":"new-terminal", "workspace_id":"w1", "tab_id":"new", "focused":false, "agent_status":"unknown", "revision":1})).unwrap(),
                })
            }
            Method::PaneCloseIfIdentity(p) => {
                assert!(p.close_exited_shell);
                assert!(!p.allow_busy_shell);
                assert_eq!(p.expected_terminal_id, "terminal");
                if self.refuse_close {
                    identity_changed()
                } else {
                    Ok(ResponseResult::Ok {})
                }
            }
            Method::AgentStart(p) => {
                let mut info = native_info("new-pane", "codex", "session");
                info.terminal_id = "new-terminal".into();
                info.name = Some(p.name.clone());
                info.launch_pending = true;
                self.starts.lock().unwrap().push(p);
                Ok(ResponseResult::AgentStarted {
                    agent: info,
                    argv: vec![],
                })
            }
            Method::PaneReportAgentSession(p) => {
                assert_eq!(p.agent_session_id.as_deref(), Some("session"));
                Ok(ResponseResult::Ok {})
            }
            other => MissingNative.request(other),
        }
    }
}

#[test]
fn codexup_room_resume_preserves_the_session_queue_and_capture_and_refuses_an_unsafe_close() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    for refuse_close in [false, true] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        let request = queue(&mut worker, room, agent, "saved task");
        let hooks = dir.join(".codex/hooks.json");
        let document = merge_hooks(
            json!({}),
            HookContract::for_provider(Provider::Codex),
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        std::fs::write(&hooks, serde_json::to_vec(&document).unwrap()).unwrap();
        let starts = Arc::new(Mutex::new(Vec::new()));
        worker.transport = Box::new(RecoveryNative {
            starts: Arc::clone(&starts),
            refuse_close,
        });
        let result = worker.resume_room(room).unwrap();
        assert_eq!(
            result["agents"][0]["outcome"],
            if refuse_close { "blocked" } else { "resuming" }
        );
        let saved = worker.state.agent(agent).unwrap();
        assert_eq!(
            saved.runtime_identity.session_id.as_deref(),
            Some("session")
        );
        assert_eq!(saved.runtime_identity.launch_id.as_deref(), Some("launch"));
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        let launched = starts.lock().unwrap();
        assert_eq!(launched.len(), usize::from(!refuse_close));
        if !refuse_close {
            let args = &launched[0].args;
            assert_eq!(&args[..2], ["resume", "session"]);
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["-c", "check_for_update_on_startup=false"]),
                "{args:?}"
            );
            assert_eq!(
                saved.runtime_identity.terminal_id.as_deref(),
                Some("new-terminal")
            );
            assert!(saved.startup.as_ref().unwrap().preserve_on_failure);
        }
        drop(launched);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn codexup_room_outage_survives_restart_with_one_fact_and_resolves_when_provider_recovers() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    let orch = state
        .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(orch, room).unwrap();
    state.report_missing_binary(room, "codex", 100).unwrap();
    state.block_missing_binary(agent, "codex", 101).unwrap();
    worker.save(state).unwrap();
    drop(worker);
    let mut recovered = Worker::open(dir.clone(), Box::new(MissingNative)).unwrap();
    recovered.poll().unwrap();
    let state = &recovered.state;
    assert_eq!(
        state
            .notices(room)
            .filter(|p| p.text.starts_with("BLOCKED"))
            .count(),
        1
    );
    let value = serde_json::to_value(state).unwrap();
    assert_eq!(
        value["room_digests"][orch.0.to_string()]["facts"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(state.room_status(room), RuntimeStatus::Blocked);
    let mut ready = native_info("pane", "codex", "session");
    ready.name = Some(format!("bus-r{}-a{}", room.0, agent.0));
    ready.interactive_ready = true;
    recovered.transport = Box::new(FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList {
            agents: vec![ready],
        })]
        .into(),
        calls: Arc::default(),
        state_path: dir.join("state.json"),
    });
    recovered.poll().unwrap();
    assert_eq!(recovered.state.agent(agent).unwrap().wait_reason(), None);
    assert_eq!(recovered.state.room_status(room), RuntimeStatus::Idle);
    let value = serde_json::to_value(&recovered.state).unwrap();
    assert!(
        value["room_digests"][orch.0.to_string()]["facts"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.starts_with("provider-block:")),
        "a recovered provider must resolve its blocker while preserving worker availability facts"
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codexup_pending_restore_tracks_the_new_terminal_before_provider_detection() {
    for foreign in [false, true] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        let mut pending = native_info(
            "restored-pane",
            "codex",
            if foreign {
                "foreign-session"
            } else {
                "session"
            },
        );
        pending.name = Some(format!("bus-r{}-a{}", room.0, agent.0));
        pending.terminal_id = "restored-terminal".into();
        pending.agent = None;
        pending.launch_pending = true;
        pending.interactive_ready = false;
        worker.transport = Box::new(FakeTransport {
            replies: vec![Ok(ResponseResult::AgentList {
                agents: vec![pending],
            })]
            .into(),
            calls: Arc::default(),
            state_path: dir.join("state.json"),
        });
        worker.poll().unwrap();
        let saved = worker.state.agent(agent).unwrap();
        assert_eq!(
            saved.status,
            if foreign {
                RuntimeStatus::Unavailable
            } else {
                RuntimeStatus::Launching
            }
        );
        assert_eq!(
            saved.runtime_identity.terminal_id.as_deref(),
            Some(if foreign {
                "terminal"
            } else {
                "restored-terminal"
            })
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
