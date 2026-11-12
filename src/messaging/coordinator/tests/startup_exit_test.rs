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
