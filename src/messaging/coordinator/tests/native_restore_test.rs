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

#[test]
fn deferred_terminal_rebind_does_not_hide_the_next_native_failure() {
    struct Frames {
        pending: VecDeque<schema::AgentInfo>,
        current: NativeFailure,
    }
    impl Transport for Frames {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if matches!(method, Method::AgentList(_)) {
                self.current.info = self.pending.pop_front().unwrap();
                return self.current.request(method);
            }
            self.current.request(method)
        }
    }
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let (mut worker, agent, dir) = failed_native(provider, provider == Provider::Codex, false);
        let prior = worker.state.agent(agent).unwrap().clone();
        let kind = launch::provider_kind(provider);
        let session = prior.runtime_identity.session_id.clone();
        let mut row = json!({
            "terminal_id":"next-terminal", "pane_id":"pane", "name":"bus-r1-a2",
            "workspace_id":"w1", "tab_id":"t1", "focused":false, "revision":1,
            "agent_status":"unknown", "launch_pending":true,
            "agent_session":session.as_ref().map(|value| json!({
                "source":format!("herdr:{kind}"), "agent":kind, "kind":"id", "value":value
            }))
        });
        let deferred = serde_json::from_value(row.clone()).unwrap();
        row["launch_pending"] = json!(false);
        row["native_resume"] = json!({
            "name":"bus-r1-a2", "agent":kind, "session_id":session,
            "command":format!("{kind} restored"),
            "error":"shell failed to start: missing-shell (ENOENT)"
        });
        let failed: schema::AgentInfo = serde_json::from_value(row.clone()).unwrap();
        row.as_object_mut().unwrap().remove("native_resume");
        row["agent"] = json!(kind);
        row["agent_status"] = json!("idle");
        row["interactive_ready"] = json!(true);
        let healthy = serde_json::from_value(row).unwrap();
        worker.transport = Box::new(Frames {
            pending: vec![deferred, failed.clone()].into(),
            current: NativeFailure {
                info: failed.clone(),
            },
        });
        let infos = worker.poll().unwrap().unwrap();
        worker
            .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
            .unwrap();
        let rebound = worker.state.agent(agent).unwrap();
        assert_eq!(
            rebound.runtime_identity.terminal_id.as_deref(),
            Some("next-terminal")
        );
        assert_eq!(
            rebound.startup, prior.startup,
            "deferred evidence must not rearm startup"
        );
        assert_eq!(rebound.actionable_error, prior.actionable_error);

        let infos = worker.poll().unwrap().unwrap();
        worker
            .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
            .unwrap();
        let current = worker.state.agent(agent).unwrap().clone();
        let error = current.actionable_error.as_deref().unwrap();
        assert!(
            error.contains("shell failed to start") && error.contains("ENOENT"),
            "{provider:?}: {error}"
        );
        assert_eq!(
            current.startup.as_ref().unwrap().command,
            format!("{kind} restored")
        );
        assert!(current.startup.as_ref().unwrap().reported);
        assert_ne!(current.actionable_error, prior.actionable_error);

        // Client reattach and repeated observations of this same native attempt
        // preserve its episode; real confirmation then releases its error.
        drop(worker);
        let mut worker = Worker::open(
            dir.clone(),
            Box::new(Frames {
                pending: vec![failed.clone(), healthy].into(),
                current: NativeFailure { info: failed },
            }),
        )
        .unwrap();
        let infos = worker.poll().unwrap().unwrap();
        worker
            .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
            .unwrap();
        let repeated = worker.state.agent(agent).unwrap();
        assert_eq!(repeated.startup, current.startup);
        assert_eq!(repeated.actionable_error, current.actionable_error);
        let infos = worker.poll().unwrap().unwrap();
        worker
            .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
            .unwrap();
        let confirmed = worker.state.agent(agent).unwrap();
        assert_eq!(confirmed.status, RuntimeStatus::Idle);
        assert!(confirmed.startup.is_none());
        assert!(confirmed.actionable_error.is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn confirmed_new_native_attempt_clears_the_previous_startup_error() {
    struct Frames(VecDeque<schema::AgentInfo>);
    impl Transport for Frames {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentList(_) => Ok(ResponseResult::AgentList {
                    agents: vec![self.0.pop_front().unwrap()],
                }),
                other => panic!("unexpected native recovery operation: {other:?}"),
            }
        }
    }
    let (mut worker, agent, dir) = failed_native(Provider::ClaudeCode, false, false);
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .is_some());
    let mut launching = json!({"terminal_id":"recovered-terminal","pane_id":"pane","name":"bus-r1-a2",
        "workspace_id":"w1","tab_id":"t1","focused":false,"revision":1,"agent_status":"unknown","launch_pending":true,
        "agent_session":{"source":"herdr:claude","agent":"claude","kind":"id","value":"session"},
        "native_resume":{"name":"bus-r1-a2","agent":"claude","session_id":"session","command":"claude --resume session"}});
    let first = serde_json::from_value(launching.clone()).unwrap();
    launching.as_object_mut().unwrap().remove("native_resume");
    launching["agent"] = json!("claude");
    launching["agent_status"] = json!("idle");
    launching["launch_pending"] = json!(false);
    launching["interactive_ready"] = json!(true);
    worker.transport = Box::new(Frames(
        vec![first, serde_json::from_value(launching).unwrap()].into(),
    ));
    for _ in 0..2 {
        let infos = worker.poll().unwrap().unwrap();
        worker
            .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
            .unwrap();
    }
    let recovered = worker.state.agent(agent).unwrap();
    assert_eq!(recovered.status, RuntimeStatus::Idle);
    assert!(recovered.startup.is_none());
    assert!(
        recovered.actionable_error.is_none(),
        "a real confirmed recovery must clear its prior startup failure"
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
