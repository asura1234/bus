use super::*;

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
fn startup_failure_error_survives_restart_without_duplication_or_losing_queued_work() {
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
    assert_eq!(worker.state.notices(room).count(), 0);
    let failed = worker.state.agent(agent).unwrap();
    assert!(failed.startup.as_ref().unwrap().reported);
    let error = failed.actionable_error.clone().unwrap();
    assert!(error.contains("Command:") && error.contains("Last screen lines:"));
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
    assert_eq!(worker.state.notices(room).count(), 0);
    let failed = worker.state.agent(agent).unwrap();
    assert!(failed.startup.as_ref().unwrap().reported);
    assert_eq!(failed.actionable_error.as_deref(), Some(error.as_str()));
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
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

#[test]
fn startup_failure_keeps_queued_work_past_the_queued_stall_window() {
    let (mut worker, agent, room, dir) = pending(Provider::Codex);
    let mut state = worker.state.clone();
    let submitted = io::now_ms();
    let request = state
        .submit_message_with(
            room,
            Draft {
                text: "pending work".into(),
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
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    worker
        .expire_queued_requests(submitted + 10 * 60 * 1000)
        .unwrap();
    let after = worker.state.request(request).unwrap();
    assert_eq!(
        after.phase,
        RequestPhase::Queued,
        "work the startup blocker says is retained was closed: {:?}",
        after.failure_reason
    );
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap()
        .starts_with("Startup failed"));
    // Confirmation releases the hold with a fresh opportunity to dispatch,
    // even when Idle did not change during the failed launch.
    let recovered = submitted + 11 * 60_000;
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, recovered)
        .unwrap();
    worker.state.set_agent_startup(agent, None).unwrap();
    worker.expire_queued_requests(recovered).unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    worker
        .expire_queued_requests(recovered + crate::messaging::model::QUEUED_STALL_MS + 1)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned,
        "confirmed startup must release the hold on normal queue expiry"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
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
            Some(SHELL_PROMPT_WAIT_ERROR),
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
            Some(SHELL_PROMPT_WAIT_ERROR)
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
