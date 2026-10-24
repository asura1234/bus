use super::*;

struct NativeFailure {
    info: schema::AgentInfo,
}

impl Transport for NativeFailure {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList {
                agents: vec![self.info.clone()],
            }),
            Method::PaneGet(_) => Ok(ResponseResult::PaneInfo {
                pane: serde_json::from_value(json!({
                    "pane_id":"pane", "terminal_id":self.info.terminal_id, "workspace_id":"w1",
                    "tab_id":"t1", "focused":false, "agent_status":"unknown", "revision":1
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
            other => panic!("failed native launch must retain its owned shell: {other:?}"),
        }
    }
}

fn failed_native(
    provider: Provider,
    sessionless: bool,
    historical: bool,
) -> (Worker, AgentId, PathBuf) {
    let (mut worker, agent, _, dir, _) = fixture(provider, vec![]);
    let mut state = worker.state.clone();
    if sessionless {
        let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        state.set_agent_runtime_identity(agent, identity).unwrap();
    }
    if historical {
        state
            .record_exit_snapshot(
                agent,
                crate::messaging::model::ExitSnapshot {
                    launch_id: Some("launch".into()),
                    terminal_id: "previous-terminal".into(),
                    path: None,
                    error: None,
                    tail: "historical diagnostic".into(),
                    at_ms: 1,
                    terminal_closed: Some(true),
                },
            )
            .unwrap();
    }
    worker.save(state).unwrap();
    drop(worker);
    let kind = launch::provider_kind(provider);
    let info = serde_json::from_value(json!({
        "terminal_id":"terminal", "pane_id":"pane", "workspace_id":"w1", "tab_id":"t1",
        "focused":false, "revision":1, "agent_status":"unknown",
        "native_resume":{"name":"bus-r1-a2", "agent":kind,
            "session_id":if sessionless { None } else { Some("session") },
            "command":if sessionless { "codex" } else { "claude --resume session" },
            "error":"provider exited before startup confirmation"}
    }))
    .unwrap();
    let mut worker = Worker::open(dir.clone(), Box::new(NativeFailure { info })).unwrap();
    let infos = worker.poll().unwrap().unwrap();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
        .unwrap();
    (worker, agent, dir)
}

#[test]
fn historical_exit_snapshot_does_not_hide_a_new_native_resume_failure() {
    let (worker, agent, dir) = failed_native(Provider::ClaudeCode, false, true);
    let a = worker.state.agent(agent).unwrap();
    assert!(a
        .actionable_error
        .as_deref()
        .is_some_and(|e| e.contains("No conversation found")));
    assert_eq!(
        a.exit_snapshot.as_ref().unwrap().tail,
        "historical diagnostic"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sessionless_codex_native_restore_failure_has_an_actionable_error() {
    let (worker, agent, dir) = failed_native(Provider::Codex, true, false);
    let a = worker.state.agent(agent).unwrap();
    assert!(
        a.actionable_error.is_some(),
        "sessionless restore must be watched"
    );
    assert!(a.runtime_identity.session_id.is_none());
    assert!(a.startup.as_ref().unwrap().reported);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejected_conversation_error_requires_delete_and_recreate() {
    let (worker, agent, dir) = failed_native(Provider::ClaudeCode, false, false);
    let error = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap();
    assert!(
        error.contains("delete") && error.contains("recreat"),
        "{error}"
    );
    assert!(
        !error.contains("Inspect readiness or resume this agent"),
        "{error}"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The client owns the coordinator; the native server and its provider PTYs
/// outlive a client detach. The worker's provider exited normally while no
/// client was attached; nothing was relaunched on reattach.
struct ExitedWhileDetached;

impl Transport for ExitedWhileDetached {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![] }),
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
                    "text":"Goodbye!\nscratch% "
                }))
                .unwrap(),
            }),
            Method::PaneCloseIfIdentity(_) => Ok(ResponseResult::Ok {}),
            other => panic!("unexpected operation: {other:?}"),
        }
    }
}

#[test]
fn client_reattach_records_worker_exit_instead_of_failed_startup() {
    let (worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(ExitedWhileDetached)).unwrap();
    let infos = worker.poll().unwrap().unwrap();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
        .unwrap();
    let observed = worker.state.agent(agent).unwrap().clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        observed.exit_snapshot.is_some(),
        "an exited worker's screen must be saved; error was {:?}",
        observed.actionable_error
    );
    assert!(observed.startup.is_none());
    assert!(observed.actionable_error.is_none());
    assert_eq!(observed.exit_snapshot.unwrap().terminal_closed, Some(true));
    assert!(observed.runtime_identity.pane_id.is_none());
}

#[test]
fn native_resume_correlation_rejects_foreign_and_ambiguous_ownership() {
    struct ListOnly(Vec<schema::AgentInfo>);
    impl Transport for ListOnly {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentList(_) => Ok(ResponseResult::AgentList {
                    agents: self.0.clone(),
                }),
                _ => Ok(ResponseResult::Ok {}),
            }
        }
    }
    for case in [
        "owner",
        "provider",
        "session",
        "live-session",
        "ambiguous",
        "other-owner",
    ] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
        let mut row = json!({"terminal_id":"restored-terminal", "pane_id":"restored-pane", "workspace_id":"w1", "tab_id":"t1", "focused":false,"revision":1,"agent_status":"unknown",
            "native_resume":{"name":"bus-r1-a2","agent":"claude","session_id":"session","command":"claude --resume session","error":"failed"}});
        match case {
            "owner" => row["native_resume"]["name"] = json!("foreign"),
            "provider" => row["native_resume"]["agent"] = json!("codex"),
            "session" => row["native_resume"]["session_id"] = json!("foreign"),
            "live-session" => {
                row["agent_session"] =
                    json!({"source":"herdr:claude","agent":"claude","kind":"id","value":"foreign"})
            }
            "other-owner" => {
                let mut state = worker.state.clone();
                let other = state
                    .create_agent(room, "other", Provider::ClaudeCode, dir.clone(), None)
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
                worker.save(state).unwrap();
            }
            _ => {}
        }
        let mut rows = vec![serde_json::from_value::<schema::AgentInfo>(row).unwrap()];
        if case == "ambiguous" {
            let mut second = rows[0].clone();
            second.terminal_id = "second-terminal".into();
            second.pane_id = "second-pane".into();
            rows.push(second);
        }
        worker.transport = Box::new(ListOnly(rows));
        worker.poll().unwrap();
        let observed = worker.state.agent(agent).unwrap();
        assert_eq!(
            observed.runtime_identity.terminal_id.as_deref(),
            Some("terminal"),
            "{case}"
        );
        assert!(observed.startup.is_none(), "{case}");
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// After a Bus restart the restored PTY keeps its pane ID but gets a new
/// terminal ID. The resumed CLI rejects the saved conversation and exits
/// before the coordinator's first poll, so the pane is back at its shell.
struct RejectedResumeInNewTerminal;

impl Transport for RejectedResumeInNewTerminal {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList {
                agents: vec![serde_json::from_value(json!({
                    "terminal_id":"restored-terminal", "pane_id":"pane", "name":"bus-r1-a2",
                    "native_resume":{"name":"bus-r1-a2","agent":"claude","session_id":"session","command":"claude --resume session","error":"provider exited before startup confirmation"},
                    "agent_status":"unknown", "workspace_id":"w1", "tab_id":"t1",
                    "focused":false, "revision":1
                }))
                .unwrap()],
            }),
            Method::PaneGet(_) => Ok(ResponseResult::PaneInfo {
                pane: serde_json::from_value(json!({
                    "pane_id":"pane", "terminal_id":"restored-terminal", "workspace_id":"w1",
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
fn restart_resume_rejection_in_reallocated_terminal_reports_provider_screen() {
    let (worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(RejectedResumeInNewTerminal)).unwrap();
    let infos = worker.poll().unwrap().unwrap();
    worker
        .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
        .unwrap();
    let failed = worker.state.agent(agent).unwrap();
    let error = failed.actionable_error.clone().unwrap_or_default();
    let pane = failed.runtime_identity.pane_id.clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        error.contains("No conversation found with session ID: session"),
        "the provider's rejection must reach the actionable error: {error}"
    );
    assert_eq!(
        pane.as_deref(),
        Some("pane"),
        "the restored pane still exists and must stay addressable for resume/delete"
    );
}
