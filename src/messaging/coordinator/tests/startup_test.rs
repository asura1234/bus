use super::*;

#[test]
fn cursor_and_grok_worker_bootstraps_stay_out_of_room_history() {
    for provider in [Provider::Cursor, Provider::Grok] {
        let (worker, agent, room, dir, _) = fixture(provider, vec![]);
        let mut state = worker.state.clone();
        let hooks = crate::agents::providers::hook_json::HookContext {
            binary: dir.join("bus"),
            spool: dir.join("callbacks/worker-bootstrap"),
            launch_id: "worker-bootstrap".into(),
        };
        std::fs::create_dir_all(&hooks.spool).unwrap();
        let input = AddAgent {
            room,
            name: "author".into(),
            provider,
            cwd: dir.to_string_lossy().into_owned(),
            extra_args: String::new(),
            consent_project_hooks: true,
        };
        let mut prepared = launch::PreparedLaunch {
            cwd: dir.clone(),
            args: Vec::new(),
            adopted_session: None,
            env: std::collections::HashMap::new(),
        };
        worker
            .prepare_agent_prompt(&input, agent, &mut state, None, &mut prepared, &hooks)
            .unwrap();
        assert!(state.queued_requests(agent).is_empty());
        assert_eq!(state.requests().count(), 0);
        assert_eq!(state.notices(room).count(), 0);
        let prompt = std::fs::read_to_string(hooks.spool.join("system-prompt.md")).unwrap();
        assert!(prompt.contains("Bus messaging"));
        assert!(!prompt.contains("Grok orchestrator instructions"));
        assert!(!prompt.contains("## Authored room instructions"));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[derive(Default)]
struct FailedLaunchNative {
    state_path: PathBuf,
    close_failures: usize,
    screen_terminal: Option<String>,
    shell_wait_code: Option<&'static str>,
    launch_args: Option<Arc<Mutex<Vec<String>>>>,
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
                if let Some(args) = &self.launch_args {
                    *args.lock().unwrap() = params.args.clone();
                }
                if let Some(code) = self.shell_wait_code {
                    return Err(TransportError { code: Some(code.into()), message: SHELL_PROMPT_WAIT_ERROR.into(), definitely_rejected: true });
                }
                let mut info = native_info("pane", "codex", "unused");
                info.name = Some(params.name);
                info.agent_session = None;
                info.launch_pending = true;
                Ok(ResponseResult::AgentStarted {
                    agent: info,
                    argv: vec!["bus-provider-does-not-exist".into(), "--no-daemon".into()],
                })
            }
            // Simulate an exited or absent native launch.
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![] }),
            Method::PaneGet(_) => Ok(ResponseResult::PaneInfo {
                pane: serde_json::from_value(json!({
                    "pane_id":"pane", "terminal_id":self.screen_terminal.as_deref().unwrap_or("terminal"),
                    "workspace_id":"w1", "tab_id":"t1", "focused":false,
                    "agent_status":"unknown", "revision":1
                })).unwrap(),
            }),
            Method::AgentStartupSnapshot(params) => {
                if self.screen_terminal.is_some() {
                    return Err(TransportError { code: Some("agent_identity_changed".into()),
                        message: "Launch terminal is no longer available at its recorded pane".into(),
                        definitely_rejected: true });
                }
                Ok(ResponseResult::AgentStartupSnapshot { snapshot: serde_json::from_value(json!({
                    "pane_id": params.pane_id, "terminal_id": params.expected_terminal_id,
                    "native_agent":"codex", "native_name":"bus-r1-a2", "native_status":"working",
                    "launch_pending":true, "interactive_ready":false,
                    "screen":"› Ask Codex to do anything\n\n  GPT-6.1-Sol xhigh\n  ? for shortcuts",
                    "osc_title":"⠋ Working", "osc_progress":"4;1;", "content_revision":2
                })).unwrap() })
            }
            Method::PaneRead(_) => Ok(ResponseResult::PaneRead {
                read: serde_json::from_value(json!({
                    "pane_id":"pane", "workspace_id":"w1", "tab_id":"t1",
                    "source":"recent", "format":"text", "revision":1,
                    "text":if self.shell_wait_code.is_some() { "STARTUP QUESTION [Y/n] " } else { "zsh: command not found: bus-provider-does-not-exist\nscratch% " },
                    "truncated":false, "returned_lines":2
                })).unwrap(),
            }),
            Method::PaneCloseIfIdentity(params) => {
                let state = JsonStore::new(self.state_path.clone()).load().unwrap().unwrap();
                let agent = state.agents().find(|agent| agent.runtime_identity.terminal_id.as_deref() == Some(&params.expected_terminal_id)).unwrap();
                assert!(agent.deletion_pending);
                assert!(agent.startup.as_ref().is_some_and(|startup| startup.reported),
                    "startup failure marker must be durable before normal deletion starts");
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
    assert_eq!(worker.state.notices(room).count(), 0);
    let broken = worker
        .state
        .agents()
        .find(|agent| agent.name == "broken")
        .unwrap();
    let error = broken.actionable_error.as_deref().unwrap();
    assert!(
        !error.contains("missing or unknown conversation"),
        "{error}"
    );
    for text in ["codex", "bus-provider-does-not-exist", "command not found"] {
        assert!(error.contains(text), "missing {text}: {error}");
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
    for provider in [
        Provider::Codex,
        Provider::ClaudeCode,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
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
        assert_eq!(worker.state.notices(room).count(), 0);
        let failed = worker.state.agent(agent).unwrap();
        assert!(failed.startup.as_ref().unwrap().reported);
        assert!(failed
            .actionable_error
            .as_deref()
            .unwrap()
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
fn startup_failure_abandonment_and_diagnostics_survive_restart() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut state = worker.state.clone();
    let request = state
        .submit_message_with(
            room,
            Draft {
                text: "QUEUED_MESSAGE_MUST_NOT_APPEAR_IN_DIAGNOSTICS".into(),
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
    assert_eq!(worker.state.notices(room).count(), 0);
    let failed = worker.state.agent(agent).unwrap();
    assert!(failed.startup.as_ref().unwrap().reported);
    let error = failed.actionable_error.clone().unwrap();
    let message = worker.state.request(request).unwrap().prompt.id;
    let detail = worker
        .state
        .request(request)
        .unwrap()
        .submission_error
        .clone()
        .unwrap();
    let status = worker.control_message_at(message, io::now_ms()).unwrap();
    assert_eq!(
        status["requests"][0]["stage"], "abandoned",
        "a definite launch failure must close never-submitted work"
    );
    assert_eq!(status["requests"][0]["reason"], "agent_startup_failed");
    assert_eq!(status["requests"][0]["detail"], detail);
    assert!(error.contains("Command:") && error.contains("Last screen lines:"));
    let diagnostics: serde_json::Value = serde_json::from_str(
        error
            .split_once("Startup diagnostics:\n")
            .expect("failure needs native diagnostics")
            .1,
    )
    .unwrap();
    assert_eq!(diagnostics["native"]["osc_title"], "⠋ Working");
    assert_eq!(diagnostics["native"]["osc_progress"], "4;1;");
    assert_eq!(diagnostics["native"]["native_agent"], "codex");
    assert_eq!(diagnostics["native"]["pane_id"], "pane");
    assert_eq!(diagnostics["native"]["terminal_id"], "terminal");
    assert!(diagnostics["native"]["screen"]
        .as_str()
        .unwrap()
        .contains("anything\n\n  GPT"));
    assert!(!error.contains("QUEUED_MESSAGE_MUST_NOT_APPEAR_IN_DIAGNOSTICS"));
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(
        detail.contains("30 seconds") && detail.contains("Resend"),
        "{detail}"
    );
    assert_eq!(
        worker
            .state
            .request(request)
            .unwrap()
            .failure_reason
            .as_deref(),
        Some("agent_startup_failed")
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
    assert_eq!(worker.state.notices(room).count(), 0);
    let failed = worker.state.agent(agent).unwrap();
    assert!(failed.startup.as_ref().unwrap().reported);
    assert_eq!(failed.actionable_error.as_deref(), Some(error.as_str()));
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(
        worker
            .state
            .request(request)
            .unwrap()
            .submission_error
            .as_deref(),
        Some(detail.as_str())
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn operational_notice_startup_failure_retains_error_without_chat() {
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
    assert!(worker.state.requests().all(|r| r.agent_id != orchestrator));
    assert_eq!(worker.state.notices(room).count(), 0);
    let error = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap();
    assert!(
        error.contains("Startup failed")
            && error.contains("Command:")
            && error.contains("Last screen lines:"),
        "{error}"
    );
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    assert!(worker.state.requests().all(|r| r.agent_id != orchestrator));
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
    assert_eq!(worker.state.notices(room).count(), 0);
    let text = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap();
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
    assert_eq!(worker.state.notices(room).count(), 0);
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap()
        .contains("Startup failed"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_stuck_failed_startup_deletion_does_not_relog_pending_on_each_retry() {
    let (mut worker, agent, _room, dir) = pending(Provider::Codex);
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        close_failures: 100,
        ..Default::default()
    });
    // Readiness failure retains the agent; deletion is an explicit action.
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    let error = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap();
    assert!(
        error.contains("Startup failed")
            && error.contains("Command:")
            && error.contains("Last screen lines:")
    );
    // Keep callsite interest global while other libtest threads exercise the
    // same log site without a thread-local subscriber.
    let _callsite_registration = tracing::Dispatch::new(tracing_subscriber::registry());
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered("bus=info", || {
        for _ in 0..5 {
            assert!(worker.delete_agent(agent, &mpsc::channel().0).is_err());
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
fn staffing_readiness_timeout_retains_agent_identity() {
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
        "timeout must preserve agent identity"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_failure_closes_queued_work_and_notifies_its_sender() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    let sender = state
        .create_agent(master, "sender", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(sender, room).unwrap();
    let request = state
        .submit_message_with(
            room,
            Draft {
                text: "pending work".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Agent(sender),
            io::now_ms(),
            true,
        )
        .unwrap()[0];
    let second = state
        .submit_message_with(
            room,
            Draft {
                text: "second queued task".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Agent(sender),
            io::now_ms(),
            true,
        )
        .unwrap()[0];
    let message = state.request(request).unwrap().prompt.id;
    worker.save(state).unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, io::now_ms())
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(
        worker
            .state
            .request(request)
            .unwrap()
            .failure_reason
            .as_deref(),
        Some("agent_startup_failed")
    );
    assert!(worker.state.queued_requests(agent).is_empty());
    assert_eq!(
        worker.state.request(second).unwrap().phase,
        RequestPhase::Abandoned
    );
    let status = worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "failed-startup-status".into(),
            method: "message.status".into(),
            params: json!({"message":message.0.to_string()}),
        },
        None,
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.result["complete"], true);
    let detail = status.result["requests"][0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("launch ended") && detail.contains("Resend"),
        "{detail}"
    );
    worker.state.retain_worker_outcomes(io::now_ms());
    let events = worker
        .state
        .worker_events_for(worker.state.request(request).unwrap());
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        events[0].1.contains("agent_startup_failed") && events[0].1.contains("Resend"),
        "{events:?}"
    );
    worker.state.retain_worker_outcomes(io::now_ms());
    assert_eq!(
        worker
            .state
            .worker_events_for(worker.state.request(request).unwrap())
            .len(),
        1
    );
    worker
        .state
        .route_worker_replies(io::now_ms() + 2_100, &BTreeSet::new())
        .unwrap();
    let notices: Vec<_> = worker
        .state
        .requests()
        .filter(|r| r.agent_id == sender)
        .collect();
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].prompt.text.contains("agent_startup_failed")
            && notices[0].prompt.text.contains("Resend"),
        "{}",
        notices[0].prompt.text
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_failure_abandons_new_sends_with_persistent_recovery_detail() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, io::now_ms())
        .unwrap();
    let mut state = worker.state.clone();
    let request = state
        .submit_message_with(
            room,
            Draft {
                text: "sent after failure".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            io::now_ms(),
            false,
        )
        .unwrap()[0];
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(
        state.request(request).unwrap().failure_reason.as_deref(),
        Some("agent_startup_failed")
    );
    let detail = state
        .request(request)
        .unwrap()
        .submission_error
        .clone()
        .unwrap();
    assert!(
        detail.contains("launch ended") && detail.contains("Resend"),
        "{detail}"
    );
    assert!(state.queued_requests(agent).is_empty());
    state.delete_agent(agent, io::now_ms()).unwrap();
    assert_eq!(
        state.request(request).unwrap().failure_reason.as_deref(),
        Some("agent_startup_failed")
    );
    assert_eq!(
        state.request(request).unwrap().submission_error.as_deref(),
        Some(detail.as_str())
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_failure_does_not_abandon_submitted_or_uncertain_work() {
    for outcome in [
        None,
        Some(SubmissionOutcome::Confirmed {
            provider_session_id: None,
            provider_turn_id: None,
        }),
        Some(SubmissionOutcome::Uncertain {
            message: "receipt lost".into(),
        }),
    ] {
        let (mut worker, agent, room, dir) = pending(Provider::Codex);
        let mut state = worker.state.clone();
        let first = state
            .submit_message_with(
                room,
                Draft {
                    text: "already submitted".into(),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                io::now_ms(),
                true,
            )
            .unwrap()[0];
        let launch = state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .launch_id
            .clone()
            .unwrap();
        state.begin_submission(first, &launch, 0).unwrap();
        if let Some(outcome) = outcome {
            state.record_submission(first, outcome).unwrap();
        }
        let before = state.request(first).unwrap().clone();
        let waiting = state
            .submit_message_with(
                room,
                Draft {
                    text: "never submitted".into(),
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
            .cleanup_failed_startups(&[], &mpsc::channel().0, io::now_ms())
            .unwrap();
        assert_eq!(worker.state.request(first).unwrap(), &before);
        assert_eq!(
            worker.state.request(waiting).unwrap().phase,
            RequestPhase::Abandoned
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn startup_failure_excludes_transient_unavailability_and_startup_dialogs() {
    for dialog in [false, true] {
        let (mut worker, agent, room, dir) = pending(Provider::Codex);
        let mut state = worker.state.clone();
        let request = state
            .submit_message_with(
                room,
                Draft {
                    text: "wait for readiness".into(),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                io::now_ms(),
                true,
            )
            .unwrap()[0];
        if dialog {
            let mut info = pending_info(&worker, agent);
            info.agent_status = schema::AgentStatus::Blocked;
            worker.save(state).unwrap();
            worker
                .cleanup_failed_startups(&[info], &mpsc::channel().0, u64::MAX)
                .unwrap();
            assert!(worker.state.agent(agent).unwrap().startup.is_none());
        } else {
            state.set_agent_startup(agent, None).unwrap();
            state
                .observe_status(agent, RuntimeStatus::Unavailable, io::now_ms())
                .unwrap();
            worker.save(state).unwrap();
            worker
                .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
                .unwrap();
        }
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        assert!(worker
            .state
            .request(request)
            .unwrap()
            .failure_reason
            .is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

const SHELL_PROMPT_WAIT_ERROR: &str = "Shell waiting for input or prompt confirmation after 30 seconds. Last visible line: STARTUP QUESTION [Y/n]. Answer any shell startup question, then resume this agent.";

#[test]
fn shell_prompt_wait_error_survives_watchdog_and_restart_with_queued_work() {
    for code in ["agent_shell_waiting", "agent_shell_starting"] {
        let (mut worker, previous, room, dir, _) = fixture(Provider::Codex, vec![]);
        let mut state = worker.state.clone();
        state.delete_agent(previous, 0).unwrap();
        worker.save(state).unwrap();
        worker.transport = Box::new(FailedLaunchNative {
            state_path: dir.join("state.json"),
            shell_wait_code: Some(code),
            ..Default::default()
        });
        let error = worker
            .add_agent(
                AddAgent {
                    room,
                    name: "shell-blocked".into(),
                    provider: Provider::Codex,
                    cwd: dir.to_string_lossy().into_owned(),
                    extra_args: String::new(),
                    consent_project_hooks: true,
                },
                None,
                &mpsc::channel().0,
            )
            .unwrap_err();
        assert!(
            error.contains("STARTUP QUESTION"),
            "native evidence reached the worker: {error}"
        );
        assert!(
            error.contains("Startup diagnostics:"),
            "shell readiness failure needs diagnostics: {error}"
        );
        let agent = worker
            .state
            .agents()
            .find(|a| a.name == "shell-blocked")
            .unwrap()
            .id;
        let identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        let submitted = io::now_ms();
        let mut state = worker.state.clone();
        let request = state
            .submit_message_with(
                room,
                Draft {
                    text: "retained pending work".into(),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                submitted,
                true,
            )
            .unwrap()[0];
        worker.save(state).unwrap();
        worker
            .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
            .unwrap();
        assert_eq!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .actionable_error
                .as_deref(),
            Some(error.as_str()),
            "watchdog must preserve the actionable shell-wait error"
        );
        worker
            .expire_queued_requests(submitted + 10 * 60_000)
            .unwrap();
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        assert_eq!(
            worker.state.agent(agent).unwrap().runtime_identity,
            identity
        );
        assert!(!worker.state.agent(agent).unwrap().deletion_pending);
        assert_eq!(worker.state.notices(room).count(), 0);
        drop(worker);
        let mut worker = Worker::open(
            dir.clone(),
            Box::new(FailedLaunchNative {
                state_path: dir.join("state.json"),
                shell_wait_code: Some(code),
                ..Default::default()
            }),
        )
        .unwrap();
        worker
            .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
            .unwrap();
        assert_eq!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .actionable_error
                .as_deref(),
            Some(error.as_str())
        );
        assert_eq!(
            worker.state.agent(agent).unwrap().runtime_identity,
            identity
        );
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        assert_eq!(worker.state.notices(room).count(), 0);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

struct ShellRetryNative {
    fallback: FailedLaunchNative,
    starts: Arc<Mutex<Vec<schema::AgentStartParams>>>,
    ready: Arc<Mutex<Option<schema::AgentInfo>>>,
    uncertain: bool,
}

impl Transport for ShellRetryNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList {
                agents: self.ready.lock().unwrap().iter().cloned().collect(),
            }),
            Method::AgentStartIfIdentity(params) => {
                let state = JsonStore::new(self.fallback.state_path.clone())
                    .load()
                    .unwrap()
                    .unwrap();
                let saved = state
                    .agents()
                    .find(|a| a.runtime_identity.pane_id.as_deref() == Some(&params.start.pane_id))
                    .unwrap();
                assert!(
                    matches!(
                        saved.startup.as_ref().unwrap().recovery,
                        Some(crate::messaging::model::StartupRecovery::Requested)
                    ),
                    "retry authority must be consumed durably before input"
                );
                assert!(
                    !saved.startup.as_ref().unwrap().reported,
                    "the retry gets a fresh startup watch"
                );
                assert_eq!(
                    saved.runtime_identity.terminal_id.as_deref(),
                    Some(params.expected_terminal_id.as_str())
                );
                self.starts.lock().unwrap().push(params.start.clone());
                if self.uncertain {
                    return Err(TransportError {
                        code: None,
                        message: "lost launch response".into(),
                        definitely_rejected: false,
                    });
                }
                let mut info = native_info(&params.start.pane_id, &params.start.kind, "unused");
                info.terminal_id = params.expected_terminal_id;
                info.name = Some(params.start.name);
                info.agent_session = None;
                info.interactive_ready = false;
                info.launch_pending = true;
                let mut argv = vec![saved.provider.executable().to_owned()];
                argv.extend(params.start.args);
                Ok(ResponseResult::AgentStarted { agent: info, argv })
            }
            other @ (Method::PaneGet(_) | Method::PaneRead(_)) => self.fallback.request(other),
            other => panic!("recovery must retain the pane and use guarded start: {other:?}"),
        }
    }
}

fn shell_blocked_first_launch(
    provider: Provider,
) -> (Worker, AgentId, RoomId, PathBuf, Vec<String>) {
    let (mut worker, previous, room, dir, _) = fixture(provider, vec![]);
    let mut state = worker.state.clone();
    state.delete_agent(previous, 0).unwrap();
    worker.save(state).unwrap();
    let args = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        shell_wait_code: Some("agent_shell_waiting"),
        launch_args: Some(Arc::clone(&args)),
        ..Default::default()
    });
    worker
        .add_agent(
            AddAgent {
                room,
                name: "shell-blocked".into(),
                provider,
                cwd: dir.to_string_lossy().into_owned(),
                extra_args: "--model 'literal model with spaces'".into(),
                consent_project_hooks: true,
            },
            None,
            &mpsc::channel().0,
        )
        .unwrap_err();
    let agent = worker
        .state
        .agents()
        .find(|a| a.name == "shell-blocked")
        .unwrap()
        .id;
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, u64::MAX)
        .unwrap();
    let args = args.lock().unwrap().clone();
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--model", "literal model with spaces"]));
    (worker, agent, room, dir, args)
}

#[test]
fn shell_wait_failure_guidance_to_resume_this_agent_actually_resumes_it() {
    for provider in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor] {
        let (mut worker, agent, room, dir, original_args) = shell_blocked_first_launch(provider);
        let identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        let request = queue(
            &mut worker,
            room,
            agent,
            "retained before answering the shell",
        );
        let starts = Arc::new(Mutex::new(Vec::new()));
        let ready = Arc::new(Mutex::new(None));
        // The developer answers; the native pane can now accept a guarded start.
        // Codex/Cursor hook confirmation legitimately happens only AFTER startup.
        assert_eq!(
            worker.state.agent(agent).unwrap().hook_setup_confirmed,
            provider == Provider::ClaudeCode
        );
        worker.transport = Box::new(ShellRetryNative {
            fallback: FailedLaunchNative {
                state_path: dir.join("state.json"),
                ..Default::default()
            },
            starts: Arc::clone(&starts),
            ready: Arc::clone(&ready),
            uncertain: false,
        });
        let resumed = worker.resume_room(room).unwrap();
        let row = resumed["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["agent_id"] == agent.0);
        assert!(
            row.is_some_and(|row| row["outcome"] == "resuming"),
            "following the error's own guidance does not resume the agent: {resumed}"
        );
        assert_eq!(
            worker.state.agent(agent).unwrap().runtime_identity,
            identity
        );
        assert_eq!(starts.lock().unwrap().len(), 1);
        assert_eq!(starts.lock().unwrap()[0].args, original_args);
        assert!(
            !worker
                .state
                .agent(agent)
                .unwrap()
                .startup
                .as_ref()
                .unwrap()
                .reported
        );
        worker.expire_queued_requests(u64::MAX).unwrap();
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        // Pending confirmation must not authorize a second provider command.
        worker.resume_room(room).unwrap();
        assert_eq!(starts.lock().unwrap().len(), 1);
        let mut info = native_info(
            "pane",
            crate::messaging::identity::provider_kind(provider).label(),
            "unused",
        );
        info.terminal_id = identity.terminal_id.clone().unwrap();
        info.name = Some(crate::messaging::identity::managed_name(room, agent));
        info.agent_session = None;
        info.interactive_ready = true;
        info.launch_pending = false;
        *ready.lock().unwrap() = Some(info);
        let infos = worker.poll().unwrap().unwrap();
        worker
            .cleanup_failed_startups(&infos, &mpsc::channel().0, io::now_ms())
            .unwrap();
        assert!(worker.state.agent(agent).unwrap().startup.is_none());
        assert!(worker.state.agent(agent).unwrap().hook_setup_confirmed);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued
        );
        assert_eq!(
            worker.state.agent(agent).unwrap().runtime_identity,
            identity
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn shell_recovery_never_replays_an_uncertain_start_after_reopen() {
    let (mut worker, agent, room, dir, _) = shell_blocked_first_launch(Provider::Codex);
    let identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    let request = queue(&mut worker, room, agent, "retain uncertain launch work");
    let starts = Arc::new(Mutex::new(Vec::new()));
    let transport = || ShellRetryNative {
        fallback: FailedLaunchNative {
            state_path: dir.join("state.json"),
            ..Default::default()
        },
        starts: Arc::clone(&starts),
        ready: Arc::new(Mutex::new(None)),
        uncertain: true,
    };
    worker.transport = Box::new(transport());
    assert_eq!(
        worker.resume_room(room).unwrap()["agents"][0]["outcome"],
        "blocked"
    );
    assert_eq!(starts.lock().unwrap().len(), 1);
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(transport())).unwrap();
    let resumed = worker.resume_room(room).unwrap();
    assert_eq!(resumed["agents"][0]["outcome"], "blocked");
    assert!(resumed["agents"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("unconfirmed"));
    assert_eq!(
        starts.lock().unwrap().len(),
        1,
        "a lost response cannot replay provider input"
    );
    worker.expire_queued_requests(u64::MAX).unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    assert_eq!(
        worker.state.agent(agent).unwrap().runtime_identity,
        identity
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shell_recovery_keeps_the_session_invalidation_guard() {
    let (mut worker, agent, room, dir, _) = shell_blocked_first_launch(Provider::Codex);
    let mut state = worker.state.clone();
    state.invalidate_agent_session(agent).unwrap();
    worker.save(state).unwrap();
    let starts = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(ShellRetryNative {
        fallback: FailedLaunchNative {
            state_path: dir.join("state.json"),
            ..Default::default()
        },
        starts: Arc::clone(&starts),
        ready: Arc::new(Mutex::new(None)),
        uncertain: false,
    });
    let resumed = worker.resume_room(room).unwrap();
    assert_eq!(resumed["agents"][0]["outcome"], "blocked");
    assert!(resumed["agents"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("ownership"));
    assert!(starts.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shell_recovery_does_not_invent_retry_authority_from_legacy_command_text() {
    let (mut worker, agent, room, dir, _) = shell_blocked_first_launch(Provider::ClaudeCode);
    let saved = worker.state.agent(agent).unwrap().startup.as_ref().unwrap();
    let legacy: crate::messaging::model::AgentStartup = serde_json::from_value(json!({
        "command":saved.command, "started_at_ms":saved.started_at_ms, "reported":true,
    }))
    .unwrap();
    let mut state = worker.state.clone();
    state.set_agent_startup(agent, Some(legacy)).unwrap();
    worker.save(state).unwrap();
    let starts = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(ShellRetryNative {
        fallback: FailedLaunchNative {
            state_path: dir.join("state.json"),
            ..Default::default()
        },
        starts: Arc::clone(&starts),
        ready: Arc::new(Mutex::new(None)),
        uncertain: false,
    });
    let resumed = worker.resume_room(room).unwrap();
    assert_eq!(resumed["agents"][0]["outcome"], "blocked");
    assert_eq!(resumed["agents"][0]["reason"], "No saved provider session");
    assert!(
        starts.lock().unwrap().is_empty(),
        "display command text does not prove unsubmitted input"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[path = "native_restore_test.rs"]
mod native_restore;

struct ResumedSessionShellWaitNative {
    fallback: FailedLaunchNative,
}

impl Transport for ResumedSessionShellWaitNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::PaneCloseIfIdentity(_) | Method::PaneReportAgentSession(_) => {
                Ok(ResponseResult::Ok {})
            }
            other => self.fallback.request(other),
        }
    }
}

#[test]
fn resumed_session_shell_wait_preserves_queued_work() {
    use crate::agents::providers::hook_json::{merge_hooks, HookContract};
    for code in ["agent_shell_waiting", "agent_shell_starting"] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        let request = queue(
            &mut worker,
            room,
            agent,
            "work waiting for resumed conversation",
        );
        let hooks = dir.join(".codex/hooks.json");
        let document = merge_hooks(
            json!({}),
            HookContract::for_provider(Provider::Codex),
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        std::fs::write(&hooks, serde_json::to_vec(&document).unwrap()).unwrap();
        let sent_args = Arc::new(Mutex::new(Vec::new()));
        worker.transport = Box::new(ResumedSessionShellWaitNative {
            fallback: FailedLaunchNative {
                state_path: dir.join("state.json"),
                shell_wait_code: Some(code),
                launch_args: Some(Arc::clone(&sent_args)),
                ..Default::default()
            },
        });
        let result = worker.resume_room(room).unwrap();
        assert_eq!(result["agents"][0]["outcome"], "blocked", "{result}");
        assert!(
            result["agents"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("STARTUP QUESTION"),
            "{result}"
        );
        worker
            .cleanup_failed_startups(&[], &mpsc::channel().0, io::now_ms())
            .unwrap();
        worker.expire_queued_requests(u64::MAX).unwrap();
        drop(worker);
        let worker = reopen_saved(&dir);
        let actual = worker.state.request(request).unwrap().clone();
        let startup = worker.state.agent(agent).unwrap().startup.clone().unwrap();
        assert!(startup.reported);
        assert!(startup.failure.is_none());
        assert_eq!(
            startup.recovery,
            Some(crate::messaging::model::StartupRecovery::Unsubmitted {
                args: sent_args.lock().unwrap().clone(),
            })
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(actual.phase, RequestPhase::Queued,
            "{code}: recoverable shell wait must not close queued work; startup={startup:?}; detail={:?}", actual.submission_error);
    }
}

#[test]
fn startup_failure_notification_is_deduplicated_after_restart() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    let sender = state
        .create_agent(master, "sender", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(sender, room).unwrap();
    let request = state
        .submit_message_with(
            room,
            Draft {
                text: "queued work before failure".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Agent(sender),
            io::now_ms(),
            true,
        )
        .unwrap()[0];
    worker.save(state).unwrap();
    worker
        .cleanup_failed_startups(&[], &mpsc::channel().0, io::now_ms())
        .unwrap();
    let failed_at = worker
        .state
        .request(request)
        .unwrap()
        .completed_at_ms
        .unwrap();
    let mut state = worker.state.clone();
    state
        .route_worker_replies(failed_at + 2_100, &BTreeSet::new())
        .unwrap();
    worker.save(state).unwrap();
    let notices: Vec<_> = worker
        .state
        .requests()
        .filter(|r| r.agent_id == sender)
        .map(|r| r.id)
        .collect();
    assert_eq!(notices.len(), 1);
    drop(worker);
    let mut worker = reopen_saved(&dir);
    for _ in 0..3 {
        let mut state = worker.state.clone();
        state
            .route_worker_replies(failed_at + 2_200, &BTreeSet::new())
            .unwrap();
        worker.save(state).unwrap();
    }
    assert_eq!(
        worker
            .state
            .requests()
            .filter(|r| r.agent_id == sender)
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        notices
    );
    assert_eq!(
        worker
            .state
            .worker_events_for(worker.state.request(request).unwrap())
            .len(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_failure_preserves_generated_system_prompt_across_restart() {
    for (provider, args) in [
        (Provider::Cursor, ""),
        (
            Provider::Codex,
            "resume 01a10f9e-71ac-79e2-81b2-56f26341e7e4",
        ),
    ] {
        let (mut worker, _, room, dir, _) = fixture(provider, vec![]);
        worker.transport = Box::new(ResumedSessionShellWaitNative {
            fallback: FailedLaunchNative {
                state_path: dir.join("state.json"),
                ..Default::default()
            },
        });
        worker
            .add_agent(
                AddAgent {
                    room,
                    name: "bootstrap".into(),
                    provider,
                    cwd: dir.to_string_lossy().into_owned(),
                    extra_args: args.into(),
                    consent_project_hooks: true,
                },
                None,
                &mpsc::channel().0,
            )
            .unwrap();
        let agent = worker
            .state
            .agents()
            .find(|a| a.name == "bootstrap")
            .unwrap()
            .id;
        let prompt = worker.state.queued_requests(agent).first().copied();
        if provider == Provider::Codex {
            assert_eq!(
                worker.state.request(prompt.unwrap()).unwrap().prompt.author,
                Author::Developer
            );
        } else {
            assert!(prompt.is_none());
        }
        let launch = worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .launch_id
            .clone()
            .unwrap();
        let prompt_path = dir
            .join("callbacks")
            .join(launch)
            .join(orchestrator::PROMPT_FILE);
        let prompt_text = std::fs::read_to_string(&prompt_path).unwrap();
        let send = queue(&mut worker, room, agent, "sender-authored work");
        worker
            .cleanup_failed_startups(&[], &mpsc::channel().0, io::now_ms())
            .unwrap();
        let late = queue(&mut worker, room, agent, "new send after startup failure");
        drop(worker);
        let worker = reopen_saved(&dir);
        let generated = prompt.map(|id| worker.state.request(id).unwrap().clone());
        let queued = worker.state.queued_requests(agent).to_vec();
        let sends: Vec<_> = [send, late]
            .into_iter()
            .map(|id| worker.state.request(id).unwrap().clone())
            .collect();
        assert_eq!(std::fs::read_to_string(&prompt_path).unwrap(), prompt_text);
        assert!(!prompt_path
            .with_file_name("launch-prompt-attempted")
            .exists());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        match generated {
            Some(generated) => {
                assert_eq!(
                    generated.phase,
                    RequestPhase::Queued,
                    "{provider:?}: generated first turn must survive startup failure"
                );
                assert_eq!(queued, vec![prompt.unwrap()]);
            }
            None => assert!(queued.is_empty()),
        }
        for send in sends {
            assert_eq!(send.phase, RequestPhase::Abandoned);
            assert_eq!(send.failure_reason.as_deref(), Some("agent_startup_failed"));
        }
    }
}

#[test]
fn startup_launch_rejection_unsupported_shell_closes_new_sender_work() {
    let (mut worker, previous, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    state.delete_agent(previous, 0).unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(FailedLaunchNative {
        state_path: dir.join("state.json"),
        shell_wait_code: Some("agent_shell_integration_unavailable"),
        ..Default::default()
    });
    worker
        .add_agent(
            AddAgent {
                room,
                name: "unsupported".into(),
                provider: Provider::Codex,
                cwd: dir.to_string_lossy().into_owned(),
                extra_args: String::new(),
                consent_project_hooks: true,
            },
            None,
            &mpsc::channel().0,
        )
        .unwrap_err();
    let agent = worker
        .state
        .agents()
        .find(|a| a.name == "unsupported")
        .unwrap()
        .id;
    let request = queue(
        &mut worker,
        room,
        agent,
        "work after known launch rejection",
    );
    let request_state = worker.state.request(request).unwrap().clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        request_state.phase,
        RequestPhase::Abandoned,
        "a definitively unsupported shell cannot launch and must close never-submitted sender work"
    );
    assert_eq!(
        request_state.failure_reason.as_deref(),
        Some("agent_startup_failed")
    );
}

#[test]
fn startup_launch_rejection_editor_failure_closes_saved_session_work_promptly() {
    struct NoScreen;
    impl Transport for NoScreen {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            panic!("bound conversation must not be read for editor failure: {method:?}")
        }
    }
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker
        .watch_agent_startup(agent, vec!["codex".into()])
        .unwrap();
    let request = queue(&mut worker, room, agent, "queued task");
    let mut info = serde_json::to_value(native_info("pane", "codex", "session")).unwrap();
    info["launch_pending"] = json!(true);
    info["launch_input_error"] =
        json!("Shell transformed the launch line twice; Enter was not sent");
    let info = serde_json::from_value(info).unwrap();
    worker.transport = Box::new(NoScreen);
    let start = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms;
    worker
        .cleanup_failed_startups(&[info], &mpsc::channel().0, start + 1)
        .unwrap();
    let state = worker.state.request(request).unwrap();
    assert_eq!(state.phase, RequestPhase::Abandoned);
    assert_eq!(
        state.failure_reason.as_deref(),
        Some("agent_startup_failed")
    );
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap()
        .contains("transformed the launch line twice"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn definite_launch_rejection_vanished_pane_abandons_new_sender_work() {
    use crate::messaging::native::BusTransport;
    use crate::protocol::api::client::ConnectionTarget;
    struct VanishedPane {
        fallback: FailedLaunchNative,
        client: BusTransport,
    }
    impl Transport for VanishedPane {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if matches!(method, Method::AgentStart(_)) {
                return self.client.request_using(method, |_, request, _| {
                    Ok(json!({"id":request.id,"error":{"code":"agent_pane_not_found","message":"agent target pane pane not found"}}))
                });
            }
            self.fallback.request(method)
        }
    }
    let (mut worker, previous, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    state.delete_agent(previous, 0).unwrap();
    worker.save(state).unwrap();
    worker.transport = Box::new(VanishedPane {
        fallback: FailedLaunchNative {
            state_path: dir.join("state.json"),
            ..Default::default()
        },
        client: BusTransport::new(ConnectionTarget::SocketPath("unused".into())),
    });
    worker
        .add_agent(
            AddAgent {
                room,
                name: "vanished".into(),
                provider: Provider::Codex,
                cwd: dir.to_string_lossy().into_owned(),
                extra_args: String::new(),
                consent_project_hooks: true,
            },
            None,
            &mpsc::channel().0,
        )
        .unwrap_err();
    let agent = worker
        .state
        .agents()
        .find(|a| a.name == "vanished")
        .unwrap()
        .id;
    let request = queue(
        &mut worker,
        room,
        agent,
        "work after definite pane rejection",
    );
    let request_state = worker.state.request(request).unwrap().clone();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(request_state.phase, RequestPhase::Abandoned);
    assert_eq!(
        request_state.failure_reason.as_deref(),
        Some("agent_startup_failed")
    );
}

#[test]
fn definite_launch_rejection_after_shell_retry_allows_later_recovery() {
    struct RejectRetry {
        fallback: FailedLaunchNative,
    }
    impl Transport for RejectRetry {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentStartIfIdentity(_) => Err(TransportError {
                    code: Some("agent_shell_integration_unavailable".into()),
                    message: "no shell editor".into(),
                    definitely_rejected: true,
                }),
                other => self.fallback.request(other),
            }
        }
    }
    let (mut worker, agent, room, dir, _) = shell_blocked_first_launch(Provider::Codex);
    let request = queue(&mut worker, room, agent, "queued before failed retry");
    worker.transport = Box::new(RejectRetry {
        fallback: FailedLaunchNative {
            state_path: dir.join("state.json"),
            ..Default::default()
        },
    });
    let result = worker.resume_room(room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "blocked", "{result}");
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    let recovery = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .recovery
        .clone();
    let result = worker.resume_room(room).unwrap();
    let reason = result["agents"][0]["reason"].as_str().unwrap_or("");
    assert!(
        recovery.is_none(),
        "definite failure must clear recovery authority: {recovery:?}"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        !reason.contains("unconfirmed"),
        "the native rejection established that no input was sent: {result}"
    );
}
