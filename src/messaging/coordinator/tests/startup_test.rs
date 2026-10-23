use super::*;

#[derive(Default)]
struct FailedLaunchNative {
    state_path: PathBuf,
    close_failures: usize,
    screen_terminal: Option<String>,
}

impl Transport for FailedLaunchNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::TabCreate(_) => Ok(ResponseResult::TabCreated {
                tab: serde_json::from_value(json!({
                    "tab_id":"t1", "workspace_id":"w1", "number":1, "label":"broken",
                    "focused":false, "pane_count":1, "agent_status":"unknown"
                })).unwrap(),
                root_pane: serde_json::from_value(json!({
                    "pane_id":"pane", "terminal_id":"terminal",
                    "workspace_id":"w1", "tab_id":"t1", "focused":false,
                    "agent_status":"unknown", "revision":1
                })).unwrap(),
            }),
            Method::AgentStart(params) => {
                let mut info = native_info("pane", "codex", "unused");
                info.name = Some(params.name);
                info.agent_session = None;
                info.launch_pending = true;
                Ok(ResponseResult::AgentStarted {
                    agent: info,
                    argv: vec!["bus-provider-does-not-exist".into(), "--no-daemon".into()],
                })
            }
            // Native launch deadline elapsed and cleared the managed name.
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![] }),
            Method::PaneGet(_) => Ok(ResponseResult::PaneInfo {
                pane: serde_json::from_value(json!({
                    "pane_id":"pane", "terminal_id":self.screen_terminal.as_deref().unwrap_or("terminal"),
                    "workspace_id":"w1", "tab_id":"t1", "focused":false,
                    "agent_status":"unknown", "revision":1
                })).unwrap(),
            }),
            Method::PaneRead(_) => Ok(ResponseResult::PaneRead {
                read: serde_json::from_value(json!({
                    "pane_id":"pane", "workspace_id":"w1", "tab_id":"t1",
                    "source":"recent", "format":"text", "revision":1,
                    "text":"zsh: command not found: bus-provider-does-not-exist\nscratch% ",
                    "truncated":false, "returned_lines":2
                })).unwrap(),
            }),
            Method::PaneCloseIfIdentity(params) => {
                let state = JsonStore::new(self.state_path.clone()).load().unwrap().unwrap();
                let agent = state.agents().find(|agent| agent.runtime_identity.terminal_id.as_deref() == Some(&params.expected_terminal_id)).unwrap();
                assert!(agent.deletion_pending);
                assert!(state.notices(agent.room_id).any(|p| p.text.contains("Startup failed"))
                    || state.requests().any(|r| r.prompt.text.contains("Startup failed")),
                    "notice must be durable before normal deletion starts");
                if self.close_failures > 0 {
                    self.close_failures -= 1;
                    return Err(TransportError {
                        code: Some("terminal_stop_failed".into()), message: "cannot stop".into(),
                        definitely_rejected: true,
                    });
                }
                if params.close_exited_shell { Ok(ResponseResult::Ok {}) } else { identity_changed() }
            }
            other => panic!("unexpected failed startup operation: {other:?}"),
        }
    }
}

#[test]
fn failed_startup_reports_evidence_and_retains_identity() {
    let (mut worker, previous, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    state.delete_agent(previous, 0).unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        ..Default::default()
    });
    let (events, _) = mpsc::channel();
    worker
        .add_agent(
            AddAgent {
                room,
                name: "broken".into(),
                provider: Provider::Codex,
                cwd: dir.to_string_lossy().into_owned(),
                extra_args: String::new(),
                consent_project_hooks: true,
            },
            None,
            &events,
        )
        .unwrap();
    worker.tick().unwrap();
    assert!(
        worker.state.agents().next().is_some(),
        "startup failure must retain the registered agent"
    );
    assert!(worker
        .state
        .notices(room)
        .any(|n| n.author == Author::Bus && n.text.starts_with("Roster change: added broken")));
    let notice = worker.state.notices(room).last().unwrap();
    for text in [
        "broken",
        "codex",
        "bus-provider-does-not-exist",
        "command not found",
    ] {
        assert!(
            notice.text.contains(text),
            "missing {text}: {}",
            notice.text
        );
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

fn pending(provider: Provider) -> (Worker, AgentId, RoomId, PathBuf) {
    let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
    let mut state = worker.state.clone();
    let mut identity = state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    state.set_agent_runtime_identity(agent, identity).unwrap();
    worker.save(state).unwrap();
    worker
        .watch_agent_startup(agent, vec!["provider-command".into()])
        .unwrap();
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        ..Default::default()
    });
    (worker, agent, room, dir)
}

fn pending_info(worker: &Worker, agent: AgentId) -> schema::AgentInfo {
    let a = worker.state.agent(agent).unwrap();
    let mut info = native_info("pane", launch::provider_kind(a.provider), "unused");
    info.name = Some(format!("bus-r{}-a{}", a.room_id.0, agent.0));
    info.agent_session = None;
    info.launch_pending = true;
    info
}

#[test]
fn startup_timeout_is_bounded_for_every_provider_even_with_a_live_pending_terminal() {
    for provider in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor] {
        let (mut worker, agent, room, dir) = pending(provider);
        let info = pending_info(&worker, agent);
        let start = worker
            .state
            .agent(agent)
            .unwrap()
            .startup
            .as_ref()
            .unwrap()
            .started_at_ms;
        let (events, _) = mpsc::channel();
        worker
            .cleanup_failed_startups(std::slice::from_ref(&info), &events, start + 29_999)
            .unwrap();
        assert!(worker.state.agent(agent).is_some());
        worker
            .cleanup_failed_startups(&[info], &events, start + 30_000)
            .unwrap();
        assert!(worker.state.agent(agent).is_some());
        assert!(worker
            .state
            .notices(room)
            .last()
            .unwrap()
            .text
            .contains("within 30 seconds"));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn interactive_codex_without_session_or_hook_is_not_a_failed_startup() {
    let (mut worker, agent, _, dir) = pending(Provider::Codex);
    let mut info = pending_info(&worker, agent);
    info.launch_pending = false;
    info.interactive_ready = true;
    worker
        .cleanup_failed_startups(&[info], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert!(worker.state.agent(agent).unwrap().startup.is_none());
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert!(
        worker.state.agent(agent).is_some(),
        "a later provider exit must not become a startup failure"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_hook_consumed_before_cleanup_wins_over_an_expired_poll_snapshot() {
    let (mut worker, agent, _, dir) = pending(Provider::ClaudeCode);
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart", "session_id":"session"}),
    );
    // The callback's native binding operation succeeds; no other responses are needed.
    worker.transport = Box::new(FakeTransport {
        replies: VecDeque::new(),
        calls: Arc::new(Mutex::new(Vec::new())),
        state_path: dir.join("state.json"),
    });
    consume(&mut worker, agent, &dir);
    assert!(worker.state.agent(agent).unwrap().startup.is_none());
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert!(worker.state.agent(agent).is_some());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_failure_notice_survives_restart_without_duplication_or_losing_queued_work() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut state = worker.state.clone();
    let request = state
        .submit_message_with(
            room,
            Draft {
                text: "pending work".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            io::now_ms(),
            true,
        )
        .unwrap()[0];
    worker.save(state).unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert!(!worker.state.agent(agent).unwrap().deletion_pending);
    assert_eq!(worker.state.notices(room).count(), 1);
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    drop(worker);
    let mut worker = Worker::open(
        dir.clone(),
        Box::new(FailedLaunchNative {
            state_path: dir.join("state.json"),
            ..Default::default()
        }),
    )
    .unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert!(worker.state.agent(agent).is_some());
    assert_eq!(worker.state.notices(room).count(), 1);
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_failure_is_delivered_with_one_blocked_digest_fact_and_identity_retained() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    let orchestrator = state
        .create_agent(
            master,
            "orchestrator",
            Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, room).unwrap();
    worker.save(state).unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert!(worker.state.agent(agent).is_some());
    let request = worker
        .state
        .requests()
        .find(|r| r.agent_id == orchestrator)
        .unwrap();
    assert_eq!(request.prompt.author, Author::Bus);
    assert!(request.prompt.text.contains("Startup failed"));
    assert_eq!(worker.state.notices(room).count(), 0);
    let saved = serde_json::to_value(&worker.state).unwrap();
    let facts = saved["room_digests"][orchestrator.0.to_string()]["facts"]
        .as_object()
        .unwrap();
    assert_eq!(facts.len(), 1);
    assert!(facts.values().next().unwrap()["text"]
        .as_str()
        .unwrap()
        .starts_with("BLOCKED"));
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert_eq!(
        worker
            .state
            .requests()
            .filter(|r| r.agent_id == orchestrator)
            .count(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_watch_survives_restart_and_failed_poll_is_not_launch_failure() {
    let (worker, agent, room, dir) = pending(Provider::Codex);
    drop(worker);
    let mut worker = reopen_saved(&dir);
    assert!(
        worker.tick().is_ok(),
        "native list is retried on a later tick"
    );
    assert!(worker.state.agent(agent).is_some());
    assert_eq!(worker.state.notices(room).count(), 0);
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        ..Default::default()
    });
    worker.tick().unwrap();
    assert!(worker.state.agent(agent).is_some());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_failure_does_not_quote_a_replacement_terminal() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        screen_terminal: Some("someone-elses-terminal".into()),
        ..Default::default()
    });
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert!(worker.state.agent(agent).is_some());
    let text = &worker.state.notices(room).last().unwrap().text;
    assert!(text.contains("Screen unavailable"));
    assert!(
        !text.contains("command not found"),
        "replacement terminal text must be discarded"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn provider_waiting_at_its_own_startup_dialog_is_not_deleted_as_a_failed_startup() {
    // Native keeps a launch that reached the provider's own blocking dialog
    // (trust directory, hooks review, update chooser) without a deadline and
    // reports it to the room as needing help; that provider did start.
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut info = pending_info(&worker, agent);
    info.agent_status = schema::AgentStatus::Blocked;
    info.dialog_id = Some("trust_directory".into());
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    worker
        .cleanup_failed_startups(&[info], &mpsc::channel().0, start + 30_000)
        .unwrap();
    assert!(
        worker.state.agent(agent).is_some(),
        "a provider blocked at its startup dialog was deleted as a failed startup"
    );
    assert_eq!(worker.state.notices(room).count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn blocked_shell_without_the_expected_provider_still_expires() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut info = pending_info(&worker, agent);
    info.agent = None;
    info.agent_status = schema::AgentStatus::Blocked;
    info.dialog_id = Some("shell-startup-prompt".into());
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    worker
        .cleanup_failed_startups(&[info], &mpsc::channel().0, start + 30_000)
        .unwrap();
    assert!(worker.state.agent(agent).is_some());
    assert!(worker
        .state
        .notices(room)
        .last()
        .unwrap()
        .text
        .contains("Startup failed"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_stuck_failed_startup_deletion_does_not_relog_pending_every_tick() {
    let (mut worker, agent, _room, dir) = pending(Provider::Codex);
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        close_failures: 100,
        ..Default::default()
    });
    // Keep callsite interest global while other libtest threads exercise the
    // same log site without a thread-local subscriber.
    let _callsite_registration = tracing::Dispatch::new(tracing_subscriber::registry());
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered("bus=info", || {
        for _ in 0..5 {
            assert!(worker
                .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
                .is_err());
        }
    });
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    let text = capture.text();
    let pending = text.matches("bus.deletion.pending").count();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        pending, 1,
        "an unchanged pending deletion is re-announced at info on every retry ({pending} records in 5 ticks):\n{text}"
    );
}

#[test]
fn staffing_readiness_timeout_retains_agent_identity_and_queue() {
    let (mut worker, agent, _, dir) = pending(Provider::Codex);
    let info = pending_info(&worker, agent);
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    worker
        .cleanup_failed_startups(&[info], &mpsc::channel().0, start + 30_000)
        .unwrap();
    assert!(
        worker.state.agent(agent).is_some(),
        "slow startup was deleted at thirty seconds"
    );
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, start + 120_000)
        .unwrap();
    assert!(
        worker.state.agent(agent).is_some(),
        "timeout must preserve agent identity and queued work"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
