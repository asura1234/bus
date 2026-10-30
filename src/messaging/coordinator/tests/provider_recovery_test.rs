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
fn missing_binary_blocks_the_room_once_and_retains_queued_work() {
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
fn room_resume_preserves_the_session_queue_and_capture_and_refuses_an_unsafe_close() {
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
            assert!(!saved.startup.as_ref().unwrap().reported);
        }
        drop(launched);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn room_outage_survives_restart_with_one_notice_and_resolves_when_provider_recovers() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
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
    assert_eq!(
        state.agent(agent).unwrap().missing_binary.as_deref(),
        Some("codex")
    );
    assert!(state
        .room(room)
        .unwrap()
        .missing_provider_binaries
        .contains("codex"));
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
    assert!(recovered
        .state
        .agent(agent)
        .unwrap()
        .missing_binary
        .is_none());
    assert!(recovered
        .state
        .room(room)
        .unwrap()
        .missing_provider_binaries
        .is_empty());
    assert_eq!(
        recovered
            .state
            .notices(room)
            .filter(|p| p.text.starts_with("BLOCKED"))
            .count(),
        1
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pending_restore_tracks_the_new_terminal_before_provider_detection() {
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

#[test]
fn room_resume_clears_the_provider_blocker_after_its_blocked_agents_were_deleted() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    state.block_missing_binary(agent, "codex", 100).unwrap();
    // The operator removes the broken worker, restores codex, then runs the documented recovery.
    state.delete_agent(agent, 101).unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(MissingNative);
    worker.resume_room(room).unwrap();
    worker.poll().unwrap();
    assert_ne!(
        worker.state.room_status(room),
        RuntimeStatus::Blocked,
        "a room with no agent left on the missing provider must not stay blocked forever"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_blocker_from_a_failed_add_clears_once_room_resume_runs_with_no_held_agent() {
    // Recover persisted blockers from a formerly refused add with no saved
    // agent holding the outage. The documented resume must clear them.
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let mut state = worker.state.clone();
    state.report_missing_binary(room, "codex", 100).unwrap();
    worker.save(state).unwrap();
    let kind = crate::messaging::identity::provider_kind(Provider::ClaudeCode).label();
    let mut ready = native_info("pane", kind, "session");
    ready.name = Some(format!("bus-r{}-a{}", room.0, agent.0));
    ready.interactive_ready = true;
    worker.transport = Box::new(FakeTransport {
        replies: vec![
            Ok(ResponseResult::AgentList {
                agents: vec![ready.clone()],
            }),
            Ok(ResponseResult::AgentList {
                agents: vec![ready],
            }),
        ]
        .into(),
        calls: Arc::default(),
        state_path: dir.join("state.json"),
    });
    worker.poll().unwrap();
    let result = worker.resume_room(room).unwrap();
    assert_eq!(result["agents"].as_array().unwrap().len(), 0);
    assert_ne!(
        worker.state.room_status(room),
        RuntimeStatus::Blocked,
        "a room blocker no agent holds must not outlive the documented recovery"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn binary_outage_recovery_abandons_queued_work_when_startup_fails() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "retained failed-resume task");
    let hooks = dir.join(".codex/hooks.json");
    let document = merge_hooks(
        json!({}),
        HookContract::for_provider(Provider::Codex),
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    std::fs::write(&hooks, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut state = worker.state.clone();
    state.block_missing_binary(agent, "codex", 101).unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(RecoveryNative {
        starts: Arc::default(),
        refuse_close: false,
    });
    let result = worker.resume_room(room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "resuming");
    // The resumed provider never confirmed startup and its native launch ended.
    worker
        .cleanup_failed_startups(&[], &std::sync::mpsc::channel().0, u64::MAX)
        .unwrap();
    let saved = worker.state.agent(agent).unwrap();
    assert!(
        saved.startup.as_ref().is_some_and(|s| s.reported),
        "the reported watch persists the conclusive failed resume"
    );
    assert_eq!(
        saved.runtime_identity.session_id.as_deref(),
        Some("session")
    );
    assert_eq!(saved.runtime_identity.launch_id.as_deref(), Some("launch"));
    assert!(saved
        .actionable_error
        .as_deref()
        .unwrap()
        .contains("Startup failed"));
    worker
        .cleanup_failed_startups(&[], &std::sync::mpsc::channel().0, u64::MAX)
        .unwrap();
    worker
        .expire_queued_requests(io::now_ms() + 180_000)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    let closed = worker.state.request(request).unwrap();
    assert_eq!(
        closed.failure_reason.as_deref(),
        Some("agent_startup_failed")
    );
    assert!(closed
        .submission_error
        .as_deref()
        .unwrap()
        .contains("Resend"));
    assert_eq!(closed.prompt.text, "retained failed-resume task");
    assert!(closed.submission_boundary.is_none());
    assert_eq!(
        worker
            .state
            .notices(room)
            .filter(|n| n.text.contains("Startup failed"))
            .count(),
        0,
        "startup failures retain structured state without retired operational chat"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn refused_add_does_not_create_an_unheld_room_blocker() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let (worker, _, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let old = std::env::var_os("PATH");
    std::env::set_var("PATH", &dir);
    let result = worker.ensure_provider_binary(room, Provider::Codex);
    match old {
        Some(v) => std::env::set_var("PATH", v),
        None => std::env::remove_var("PATH"),
    }
    assert!(result.unwrap_err().contains("codex"));
    assert!(worker
        .state
        .room(room)
        .unwrap()
        .missing_provider_binaries
        .is_empty());
    assert!(!worker
        .state
        .notices(room)
        .any(|n| n.text.starts_with("BLOCKED")));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_resume_keeps_the_saved_pane_while_the_provider_binary_is_missing() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    state.block_missing_binary(agent, "codex", 100).unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(MissingNative);
    let old = std::env::var_os("PATH");
    std::env::set_var("PATH", &dir);
    let result = worker.resume_room(room);
    match old {
        Some(v) => std::env::set_var("PATH", v),
        None => std::env::remove_var("PATH"),
    }
    let result = result.unwrap();
    assert_eq!(result["agents"][0]["outcome"], "blocked");
    assert!(
        result["agents"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("missing from PATH"),
        "{result}"
    );
    let saved = worker.state.agent(agent).unwrap();
    assert_eq!(
        saved.runtime_identity.terminal_id.as_deref(),
        Some("terminal")
    );
    assert!(saved.startup.is_none());
    assert_eq!(saved.missing_binary.as_deref(), Some("codex"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deleting_the_last_missing_binary_holder_clears_the_room_outage() {
    let (worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    let second = state
        .create_agent(room, "second", Provider::Codex, dir.clone(), None)
        .unwrap();
    state.block_missing_binary(agent, "codex", 100).unwrap();
    state.block_missing_binary(second, "codex", 101).unwrap();
    state.delete_agent(agent, 102).unwrap();
    assert!(state
        .room(room)
        .unwrap()
        .missing_provider_binaries
        .contains("codex"));
    state.delete_agent(second, 103).unwrap();
    assert!(state
        .room(room)
        .unwrap()
        .missing_provider_binaries
        .is_empty());
    assert_ne!(state.room_status(room), RuntimeStatus::Blocked);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

struct GenericStartupRecovery(RecoveryNative);
impl Transport for GenericStartupRecovery {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if let Method::PaneRead(p) = method {
            return Ok(ResponseResult::PaneRead { read: serde_json::from_value(json!({
                "pane_id":p.pane_id, "workspace_id":"w1", "tab_id":"t1", "source":"recent", "format":"text",
                "revision":1, "text":"Provider startup failed before readiness\n", "truncated":false, "returned_lines":1
            })).unwrap() });
        }
        self.0.request(method)
    }
}

#[test]
fn confirmed_room_resume_clears_the_previous_startup_error() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    for refuse_close in [false, true] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        let request = queue(&mut worker, room, agent, "retained generic failure task");
        let hooks = dir.join(".codex/hooks.json");
        let document = merge_hooks(
            json!({}),
            HookContract::for_provider(Provider::Codex),
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        std::fs::write(&hooks, serde_json::to_vec(&document).unwrap()).unwrap();
        let mut state = worker.state.clone();
        state
            .set_agent_error(
                agent,
                Some("Startup failed: prior launch; identity and queue retained".into()),
            )
            .unwrap();
        state
            .set_agent_startup(
                agent,
                Some(crate::messaging::model::AgentStartup {
                    command: "codex".into(),
                    started_at_ms: 1,
                    reported: true,
                    native_terminal_id: None,
                    recovery: None,
                    failure: None,
                }),
            )
            .unwrap();
        worker.save(state).unwrap();
        worker.transport = Box::new(GenericStartupRecovery(RecoveryNative {
            starts: Arc::default(),
            refuse_close,
        }));
        let result = worker.resume_room(room).unwrap();
        assert_eq!(
            result["agents"][0]["outcome"],
            if refuse_close { "blocked" } else { "resuming" }
        );
        let saved = worker.state.agent(agent).unwrap();
        assert!(saved.missing_binary.is_none());
        if refuse_close {
            assert!(saved
                .actionable_error
                .as_deref()
                .unwrap()
                .contains("prior launch"));
            assert!(saved.startup.as_ref().unwrap().reported);
        } else {
            assert!(
                saved.actionable_error.is_none(),
                "a confirmed explicit relaunch must clear the prior startup error"
            );
            assert!(!saved.startup.as_ref().unwrap().reported);
        }
        assert_eq!(
            saved.runtime_identity.session_id.as_deref(),
            Some("session")
        );
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

struct RecoveredProviderExit {
    closes: Arc<Mutex<Vec<schema::PaneCloseIfIdentityParams>>>,
}
impl Transport for RecoveredProviderExit {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![] }),
            Method::PaneGet(p) => Ok(ResponseResult::PaneInfo {
                pane: serde_json::from_value(json!({
                    "pane_id": p.pane_id, "terminal_id": "new-terminal",
                    "workspace_id": "w1", "tab_id": "new", "focused": false,
                    "agent_status": "unknown", "revision": 1,
                    "agent_process_exited": true
                }))
                .unwrap(),
            }),
            Method::PaneRead(p) => Ok(ResponseResult::PaneRead {
                read: serde_json::from_value(json!({
                    "pane_id": p.pane_id, "workspace_id": "w1", "tab_id": "new",
                    "source": p.source, "format": "text", "revision": 1,
                    "text": "resumed provider: new fatal error", "truncated": false
                }))
                .unwrap(),
            }),
            Method::PaneCloseIfIdentity(p) => {
                self.closes.lock().unwrap().push(p);
                Ok(ResponseResult::Ok {})
            }
            other => panic!("unexpected recovered-exit operation {other:?}"),
        }
    }
}

#[test]
fn recovered_provider_records_and_closes_its_next_exit() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    struct RestorePath(Option<std::ffi::OsString>);
    impl Drop for RestorePath {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
        }
    }
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let _restore = RestorePath(std::env::var_os("PATH"));
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("codex"), "fixture executable").unwrap();
    std::env::set_var("PATH", &bin);
    let old_snapshot = dir.join("previous-exit.txt");
    std::fs::write(&old_snapshot, "previous provider exit").unwrap();
    let hooks = dir.join(".codex/hooks.json");
    let document = merge_hooks(
        json!({}),
        HookContract::for_provider(Provider::Codex),
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    std::fs::write(&hooks, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut state = worker.state.clone();
    state
        .record_exit_snapshot(
            agent,
            crate::messaging::model::ExitSnapshot {
                launch_id: Some("launch".into()),
                terminal_id: "terminal".into(),
                path: Some(old_snapshot.clone()),
                error: None,
                tail: "previous provider exit".into(),
                at_ms: 1,
                terminal_closed: Some(true),
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(RecoveryNative {
        starts: Arc::default(),
        refuse_close: false,
    });
    let result = worker.resume_room(room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "resuming");
    let mut ready = native_info("new-pane", "codex", "session");
    ready.terminal_id = "new-terminal".into();
    ready.name = Some(format!("bus-r{}-a{}", room.0, agent.0));
    ready.interactive_ready = true;
    worker.transport = Box::new(FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList {
            agents: vec![ready.clone()],
        })]
        .into(),
        calls: Arc::default(),
        state_path: dir.join("state.json"),
    });
    worker.poll().unwrap();
    worker
        .cleanup_failed_startups(&[ready], &std::sync::mpsc::channel().0, io::now_ms())
        .unwrap();
    assert!(worker.state.agent(agent).unwrap().startup.is_none());
    let closes = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(RecoveredProviderExit {
        closes: Arc::clone(&closes),
    });
    worker.poll().unwrap();
    worker.close_exited_terminals().unwrap();
    let snapshot = worker
        .state
        .agent(agent)
        .unwrap()
        .exit_snapshot
        .clone()
        .unwrap();
    let closed = closes.lock().unwrap().clone();
    assert_eq!(
        std::fs::read_to_string(&old_snapshot).unwrap(),
        "previous provider exit"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        (snapshot.terminal_id.as_str(), closed.len()),
        ("new-terminal", 1),
        "recovery must capture and close the replacement provider's own exit",
    );
    assert!(
        snapshot.tail.contains("new fatal error"),
        "the replacement exit must retain its own diagnostic screen"
    );
    assert_eq!(
        closed.len(),
        1,
        "the replacement provider's exited terminal must be handled once"
    );
    assert_eq!(closed[0].expected_terminal_id, "new-terminal");
}

#[test]
fn startup_launch_rejection_resume_tab_abandons_never_submitted_work() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    struct RejectResumeTab;
    impl Transport for RejectResumeTab {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::TabCreate(_) => Err(TransportError {
                    code: Some("tab_create_failed".into()),
                    message: "login shell is not executable".into(),
                    definitely_rejected: true,
                }),
                Method::PaneCloseIfIdentity(_) => Ok(ResponseResult::Ok {}),
                other => MissingNative.request(other),
            }
        }
    }
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let hooks = dir.join(".codex/hooks.json");
    let document = merge_hooks(
        json!({}),
        HookContract::for_provider(Provider::Codex),
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    std::fs::write(&hooks, serde_json::to_vec(&document).unwrap()).unwrap();
    let request = queue(&mut worker, room, agent, "queued task");
    worker.transport = Box::new(RejectResumeTab);
    let result = worker.resume_room(room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "blocked", "{result}");
    assert!(result["agents"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("login shell is not executable"));
    let request_state = worker.state.request(request).unwrap().clone();
    let later = queue(&mut worker, room, agent, "work after failed resume");
    let later_state = worker.state.request(later).unwrap().clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(later_state.phase, RequestPhase::Abandoned);
    assert_eq!(
        later_state.failure_reason.as_deref(),
        Some("agent_startup_failed")
    );
    assert_eq!(
        request_state.phase,
        RequestPhase::Abandoned,
        "definite resume launch creation failure must close never-submitted work"
    );
    assert_eq!(
        request_state.failure_reason.as_deref(),
        Some("agent_startup_failed")
    );
}

#[test]
fn startup_launch_rejection_uncertain_resume_tab_retains_queued_work() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    struct UncertainResumeTab;
    impl Transport for UncertainResumeTab {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::TabCreate(_) => Err(TransportError {
                    code: None,
                    message: "transport lost".into(),
                    definitely_rejected: false,
                }),
                Method::PaneCloseIfIdentity(_) => Ok(ResponseResult::Ok {}),
                other => MissingNative.request(other),
            }
        }
    }
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let hooks = dir.join(".codex/hooks.json");
    let document = merge_hooks(
        json!({}),
        HookContract::for_provider(Provider::Codex),
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    std::fs::write(&hooks, serde_json::to_vec(&document).unwrap()).unwrap();
    let request = queue(&mut worker, room, agent, "queued task");
    worker.transport = Box::new(UncertainResumeTab);
    let result = worker.resume_room(room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "blocked", "{result}");
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
