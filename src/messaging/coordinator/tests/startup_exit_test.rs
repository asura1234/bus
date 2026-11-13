use super::*;

const BOOTSTRAP_ERROR: &str = "Error: account/read failed during TUI bootstrap: account/read failed: workspace routing discovery failed (code -32603)";

struct StartupExitNative {
    state_path: PathBuf,
    exited: bool,
    terminal: &'static str,
}

impl StartupExitNative {
    fn exited(dir: &std::path::Path) -> Self {
        Self {
            state_path: dir.join("state.json"),
            exited: true,
            terminal: "terminal",
        }
    }
}

impl Transport for StartupExitNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                Ok(ResponseResult::AgentList { agents: vec![] })
            }
            Method::PaneGet(params) => Ok(ResponseResult::PaneInfo {
                pane: serde_json::from_value(json!({
                    "pane_id":params.pane_id,"terminal_id":self.terminal,
                    "workspace_id":"w1","tab_id":"t1","focused":false,
                    "agent_status":"unknown","revision":7,"agent_process_exited":self.exited
                }))
                .unwrap(),
            }),
            Method::AgentStartupSnapshot(params) => {
                assert_eq!(params.expected_terminal_id, "terminal");
                Ok(ResponseResult::AgentStartupSnapshot {
                    snapshot: serde_json::from_value(json!({
                        "pane_id":params.pane_id,"terminal_id":self.terminal,
                        "native_agent":null,"native_name":null,"native_status":"unknown",
                        "launch_pending":false,"interactive_ready":false,
                        "screen":format!("{BOOTSTRAP_ERROR}\nscratch% "),
                        "osc_title":"","osc_progress":"","content_revision":8
                    }))
                    .unwrap(),
                })
            }
            Method::PaneRead(params) => Ok(ResponseResult::PaneRead {
                read: serde_json::from_value(json!({
                    "pane_id":params.pane_id,"workspace_id":"w1","tab_id":"t1",
                    "source":params.source,"format":"text","revision":8,
                    "text":format!("{BOOTSTRAP_ERROR}\nscratch% "),"truncated":false
                }))
                .unwrap(),
            }),
            Method::PaneCloseIfIdentity(_) => {
                let saved = JsonStore::new(self.state_path.clone())
                    .load()
                    .unwrap()
                    .unwrap();
                let agent = saved.agents().find(|a| a.deletion_pending).unwrap();
                let snapshot = agent
                    .exit_snapshot
                    .as_ref()
                    .expect("snapshot durable before deletion");
                assert!(snapshot.path.as_ref().unwrap().is_file());
                assert!(agent.startup.as_ref().unwrap().reported);
                Ok(ResponseResult::Ok {})
            }
            Method::AgentGet(_) => Err(TransportError {
                timestamps: Default::default(),
                code: Some("agent_not_found".into()),
                message: "agent target pane not found".into(),
                definitely_rejected: false,
            }),
            other => {
                panic!("startup must not submit, restart, or inspect a foreign agent: {other:?}")
            }
        }
    }
}

fn failed_startup() -> (Worker, AgentId, RoomId, PathBuf, RequestId) {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut stored = json!(worker.state);
    stored["agents"][agent.0.to_string()]["hook_setup_confirmed"] = json!(false);
    // Exercise terminal diagnostics after launch retry authority is absent;
    // the independent Codex retry policy is covered by startup_retry_test.rs.
    stored["agents"][agent.0.to_string()]["startup"]["recovery"] = json!(null);
    worker
        .save(serde_json::from_value(stored).unwrap())
        .unwrap();
    worker.transport = Box::new(StartupExitNative::exited(&dir));
    let request = queue(&mut worker, room, agent, "never submit this task");
    (worker, agent, room, dir, request)
}

#[test]
fn sessionless_failed_startup_requires_replacement_but_saved_session_keeps_resume() {
    for session in [None, Some("saved-session")] {
        let (mut worker, agent, _, dir, _) = failed_startup();
        if let Some(session) = session {
            let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
            identity.session_id = Some(session.into());
            worker
                .state
                .set_agent_runtime_identity(agent, identity)
                .unwrap();
        }
        let infos = worker.poll().unwrap().unwrap();
        worker
            .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
            .unwrap();
        let failure = worker
            .state
            .agent(agent)
            .unwrap()
            .startup
            .as_ref()
            .unwrap()
            .failure
            .as_ref()
            .unwrap();
        if session.is_none() {
            assert!(failure.recovery.contains("rebrief"), "{}", failure.recovery);
            assert!(
                failure.recovery.contains("replacement agent"),
                "{}",
                failure.recovery
            );
            assert!(
                !failure.recovery.contains("before resuming"),
                "{}",
                failure.recovery
            );
        } else {
            assert!(
                failure.recovery.contains("before resuming"),
                "{}",
                failure.recovery
            );
            assert!(
                !failure.recovery.contains("rebrief"),
                "{}",
                failure.recovery
            );
        }
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

// AgentGet rejects a plain exited shell; archived evidence must remain readable.
#[test]
fn startup_exit_read_returns_saved_evidence_without_reading_current_terminal() {
    let (mut worker, agent, _, dir, _) = failed_startup();
    worker.poll().unwrap();
    let snapshot = worker
        .state
        .agent(agent)
        .unwrap()
        .exit_snapshot
        .clone()
        .expect("startup exit snapshot");
    // The fixture rejects AgentGet/AgentRead. A current replacement's screen
    // must never be read in order to return explicitly historical evidence.
    let read = worker.control_read(agent, Some("visible"), None).unwrap();
    assert_eq!(read["capture"]["source"], "exit_snapshot");
    assert_eq!(read["capture"]["at_ms"], snapshot.at_ms);
    assert_eq!(read["exit_snapshot"]["path"], json!(snapshot.path));
    assert!(read["text"]
        .as_str()
        .unwrap()
        .contains("workspace routing discovery failed"));
    assert_eq!(read["runtime"]["session_verified"], false);
    assert!(!worker.state.agent(agent).unwrap().hook_setup_confirmed);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

// Callback collection must not erase the only copy of a dead launch's error.
#[test]
fn startup_exit_error_survives_deletion_before_timeout_and_callback_gc() {
    let (mut worker, agent, _, dir, request) = failed_startup();
    let infos = worker.poll().unwrap().unwrap();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
        .unwrap();
    let path = worker
        .state
        .agent(agent)
        .unwrap()
        .exit_snapshot
        .as_ref()
        .unwrap()
        .path
        .clone()
        .unwrap();
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    assert!(!dir.join("callbacks/launch").exists());
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains(BOOTSTRAP_ERROR));
    drop(worker);
    let restored = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(restored.agent(agent).is_none());
    assert!(restored
        .request(request)
        .unwrap()
        .submission_error
        .as_deref()
        .unwrap()
        .contains("workspace routing discovery failed"));
    std::fs::remove_dir_all(dir).unwrap();
}

// A later/foreign launch cannot acquire this startup's archived evidence.
#[test]
fn startup_exit_read_rejects_snapshot_from_previous_launch() {
    let (mut worker, agent, _, dir, _) = failed_startup();
    worker.poll().unwrap();
    assert!(worker.state.agent(agent).unwrap().exit_snapshot.is_some());
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.launch_id = Some("replacement-launch".into());
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    assert!(worker.control_read(agent, Some("visible"), None).is_err());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_exit_after_timeout_preserves_diagnostics_after_deletion() {
    let (mut worker, agent, _, dir, request) = failed_startup();
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        screen: Some("Provider bootstrap is still starting".into()),
        ..Default::default()
    });
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    let pending = pending_info(&worker, agent);
    worker
        .cleanup_failed_startups(
            std::slice::from_ref(&pending),
            &mpsc::channel().0,
            start + 29_999,
        )
        .unwrap();
    worker
        .cleanup_failed_startups(&[pending], &mpsc::channel().0, start + 30_000)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(worker.state.agent(agent).unwrap().exit_snapshot.is_none());

    let completed = worker.state.request(request).unwrap().completed_at_ms;
    let latency = json!(worker.state.request(request).unwrap().latency);
    // The already observed provider exits after the startup deadline.
    worker.transport = Box::new(StartupExitNative::exited(&dir));
    let infos = worker.poll().unwrap().unwrap();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, start + 30_001)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    let error = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_ref()
        .unwrap();
    assert!(error.contains(BOOTSTRAP_ERROR), "{error}");
    assert_eq!(error.matches("Exit snapshot: ").count(), 1, "{error}");
    assert_eq!(
        worker.state.request(request).unwrap().completed_at_ms,
        completed
    );
    assert_eq!(
        json!(worker.state.request(request).unwrap().latency),
        latency
    );
    let detail = worker
        .state
        .request(request)
        .unwrap()
        .submission_error
        .clone();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, start + 30_002)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().submission_error,
        detail
    );
    let snapshot = worker
        .state
        .agent(agent)
        .unwrap()
        .exit_snapshot
        .clone()
        .unwrap();
    let path = snapshot.path.unwrap();
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains(BOOTSTRAP_ERROR));
    let read = worker.control_read(agent, Some("visible"), None).unwrap();
    assert_eq!(read["capture"]["source"], "exit_snapshot");
    assert_eq!(read["exit_snapshot"]["path"], json!(path));
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    assert!(!dir.join("callbacks/launch").exists());
    assert!(path.is_file());
    drop(worker);
    let restored = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(restored.agent(agent).is_none());
    let detail = restored
        .request(request)
        .unwrap()
        .submission_error
        .clone()
        .unwrap();
    let path_text = path.to_string_lossy().into_owned();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        detail.contains(BOOTSTRAP_ERROR) && detail.contains(&path_text),
        "captured startup error and snapshot path must survive deletion; detail={detail:?}; expected_path={path_text}"
    );
}

#[test]
fn exhausted_startup_summary_precedes_single_saved_snapshot() {
    let (mut worker, agent, _, dir, _) = failed_startup();
    let mut stored = json!(worker.state);
    stored["agents"][agent.0.to_string()]["startup"]["recovery"] =
        json!(crate::messaging::model::StartupRecovery::CodexBootstrap {
            args: vec![],
            attempts: 2,
            retry_at_ms: None,
            uncertain: false,
            exhausted: true,
        });
    worker
        .save(serde_json::from_value(stored).unwrap())
        .unwrap();
    let infos = worker.poll().unwrap().unwrap();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
        .unwrap();
    let error = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_ref()
        .unwrap();
    assert_eq!(error.matches("Exit snapshot: ").count(), 1, "{error}");
    assert!(
        error.find("after 3 tries").unwrap() < error.find("Exit snapshot: ").unwrap(),
        "{error}"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn late_startup_evidence_preserves_other_failure_diagnostics() {
    for case in [
        "older",
        "foreign_launch",
        "native_input",
        "uncertain",
        "other_failure",
    ] {
        let (mut worker, agent, _, dir, request) = failed_startup();
        worker.transport = Box::new(FailedLaunchNative {
            state_path: dir.join("state.json"),
            screen: Some("Provider bootstrap is still starting".into()),
            ..Default::default()
        });
        let start = worker
            .state
            .agent(agent)
            .unwrap()
            .startup
            .as_ref()
            .unwrap()
            .started_at_ms;
        let pending = pending_info(&worker, agent);
        worker
            .cleanup_failed_startups(
                std::slice::from_ref(&pending),
                &mpsc::channel().0,
                start + 29_999,
            )
            .unwrap();
        worker
            .cleanup_failed_startups(&[pending], &mpsc::channel().0, start + 30_000)
            .unwrap();
        let previous = worker
            .state
            .request(request)
            .unwrap()
            .submission_error
            .clone();
        let mut stored = json!(worker.state);
        let row = &mut stored["requests"][request.0.to_string()];
        match case {
            "older" => row["completed_at_ms"] = json!(start - 1),
            "foreign_launch" => row["expected_launch_id"] = json!("another-launch"),
            "native_input" => row["latency"]["native_requested_at_ms"] = json!(start),
            "uncertain" => row["uncertain_outcome"] = json!(true),
            "other_failure" => row["failure_reason"] = json!("worker_exited"),
            _ => unreachable!(),
        }
        worker
            .save(serde_json::from_value(stored).unwrap())
            .unwrap();
        worker.transport = Box::new(StartupExitNative::exited(&dir));
        let infos = worker.poll().unwrap().unwrap();
        worker
            .cleanup_failed_startups(&infos, &mpsc::channel().0, start + 30_001)
            .unwrap();
        assert!(worker.state.agent(agent).unwrap().exit_snapshot.is_some());
        assert_eq!(
            worker.state.request(request).unwrap().submission_error,
            previous,
            "{case}"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

// A positive native exit is sufficient even when no poll saw the provider.
#[test]
fn early_startup_exit_reports_provider_error_without_waiting_for_timeout() {
    let (mut worker, agent, _, dir, request) = failed_startup();
    let mut stored = json!(worker.state);
    stored["agents"][agent.0.to_string()]["startup"]["process_observed"] = json!(false);
    worker
        .save(serde_json::from_value(stored).unwrap())
        .unwrap();
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    let infos = worker.poll().unwrap().unwrap();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, start + 1)
        .unwrap();
    let a = worker.state.agent(agent).unwrap();
    assert!(a.startup.as_ref().unwrap().reported);
    let error = a.actionable_error.as_deref().unwrap();
    let snapshot = a.exit_snapshot.as_ref().unwrap();
    assert!(snapshot.path.as_ref().unwrap().is_file());
    assert!(error.contains(BOOTSTRAP_ERROR), "{error}");
    assert!(error.contains("provider exited during startup"), "{error}");
    assert!(!error.contains("within 30 seconds"), "{error}");
    let request = worker.state.request(request).unwrap();
    assert_eq!(request.phase, RequestPhase::Abandoned);
    assert!(request.latency.native_requested_at_ms.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn early_startup_exit_requires_positive_owned_native_evidence() {
    struct UnprovenExit(&'static str);
    impl Transport for UnprovenExit {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let unreadable = || TransportError {
                timestamps: Default::default(),
                code: Some("transport_unavailable".into()),
                message: "native read unavailable".into(),
                definitely_rejected: false,
            };
            match method {
                Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                    Err(unreadable())
                }
                Method::PaneGet(params) if self.0 != "transport" => Ok(ResponseResult::PaneInfo {
                    pane: serde_json::from_value(json!({
                        "pane_id":params.pane_id,
                        "terminal_id":if self.0 == "replacement" { "foreign" } else { "terminal" },
                        "workspace_id":"w1","tab_id":"t1","focused":false,
                        "agent_status":"unknown","revision":1,
                        "agent_process_exited":self.0 == "replacement"
                    }))
                    .unwrap(),
                }),
                Method::PaneGet(_) => Err(unreadable()),
                other => panic!("unproven exit must not capture or retry: {other:?}"),
            }
        }
    }
    for case in ["transport", "replacement", "absent"] {
        let (mut worker, agent, room, dir) = pending(Provider::Codex);
        let request = queue(&mut worker, room, agent, "wait for positive exit evidence");
        worker.transport = Box::new(UnprovenExit(case));
        let start = worker
            .state
            .agent(agent)
            .unwrap()
            .startup
            .as_ref()
            .unwrap()
            .started_at_ms;
        assert!(
            worker.poll().unwrap().is_none(),
            "failed list is not an empty list"
        );
        worker
            .cleanup_failed_startups(&[], &mpsc::channel().0, start + 1)
            .unwrap();
        let a = worker.state.agent(agent).unwrap();
        assert!(!a.startup.as_ref().unwrap().reported, "{case}");
        assert!(a.exit_snapshot.is_none(), "{case}");
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued,
            "{case}"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
