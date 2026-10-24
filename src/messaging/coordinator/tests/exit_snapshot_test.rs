use super::*;

const LAST_LINE: &str = "[oh-my-zsh] Would you like to update? [Y/n]";

/// The worker's pane after its provider exited on its own: the shell still
/// shows the provider's last output, then runs something else.
struct ExitedPane {
    calls: Arc<Mutex<Vec<String>>>,
    closes: Arc<Mutex<Vec<schema::PaneCloseIfIdentityParams>>>,
    /// Exit-time closes refused before the server accepts one.
    refuse_closes: usize,
    open: bool,
}

impl ExitedPane {
    fn install(f: &mut Fixture, refuse_closes: usize) -> Self {
        let pane = Self {
            calls: f.calls.clone(),
            closes: Arc::default(),
            refuse_closes,
            open: true,
        };
        f.worker.transport = Box::new(Self {
            calls: pane.calls.clone(),
            closes: pane.closes.clone(),
            refuse_closes,
            open: true,
        });
        pane
    }
}

fn scrollback() -> String {
    let mut lines: Vec<String> = (1..=230).map(|n| format!("codex output {n}")).collect();
    lines.push(String::new());
    lines.push("codex: fatal: stream disconnected".into());
    lines.push("   ".into());
    lines.push(LAST_LINE.into());
    lines.join("\n")
}

fn refused(code: &str) -> Result<ResponseResult, TransportError> {
    Err(TransportError {
        code: Some(code.into()),
        message: code.into(),
        definitely_rejected: true,
    })
}

impl Transport for ExitedPane {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![
                serde_json::from_value(json!({"terminal_id":"terminal","pane_id":"pane","agent":"claude","interactive_ready":true,"agent_status":"idle","agent_session":{"source":"herdr:claude","agent":"claude","kind":"id","value":"session"},"workspace_id":"w","tab_id":"t","focused":false,"revision":1})).unwrap()
            ] }),
            Method::PaneGet(p) if self.open && p.pane_id == "worker-pane" => Ok(ResponseResult::PaneInfo { pane: serde_json::from_value(json!({
                "pane_id":p.pane_id,"terminal_id":"worker-terminal","workspace_id":"w","tab_id":"t","focused":false,"agent_status":"unknown","revision":1,"agent_process_exited":true
            })).unwrap() }),
            Method::PaneGet(_) => refused("pane_not_found"),
            Method::PaneRead(p) if self.open && p.pane_id == "worker-pane" => {
                let text = match p.source {
                    schema::ReadSource::Visible => format!("visible screen top\n{LAST_LINE}"),
                    _ => {
                        assert_eq!(p.lines, Some(200));
                        scrollback()
                    }
                };
                Ok(ResponseResult::PaneRead { read: serde_json::from_value(json!({
                    "pane_id":p.pane_id,"workspace_id":"w","tab_id":"t","source":p.source,"format":"text","text":text,"revision":1,"truncated":false
                })).unwrap() })
            }
            Method::PaneCloseIfIdentity(p) => {
                self.closes.lock().unwrap().push(p.clone());
                if !self.open {
                    // As the server: a close of a terminal already gone succeeds,
                    // so a retry after a lost response is safe.
                    return Ok(ResponseResult::Ok {});
                }
                if !(p.close_exited_shell && p.allow_busy_shell)
                    || p.pane_id != "worker-pane"
                    || p.expected_terminal_id != "worker-terminal"
                {
                    return refused("terminal_identity_changed");
                }
                if self.refuse_closes > 0 {
                    self.refuse_closes -= 1;
                    return refused("terminal_identity_changed");
                }
                self.open = false;
                Ok(ResponseResult::Ok {})
            }
            other => Accepting(self.calls.clone()).request(other),
        }
    }
}

fn exited_worker(f: &mut Fixture) {
    let mut state = f.worker.state.clone();
    state
        .set_agent_runtime_identity(
            f.agent,
            AgentRuntimeIdentity {
                launch_id: Some("worker".into()),
                terminal_id: Some("worker-terminal".into()),
                pane_id: Some("worker-pane".into()),
                session_id: Some("worker-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(f.agent).unwrap();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 10_000)
        .unwrap();
    f.worker.save(state).unwrap();
}

fn deliver(f: &mut Fixture) {
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(io::now_ms() + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
}

fn snapshot_files(f: &Fixture) -> Vec<PathBuf> {
    std::fs::read_dir(f.dir.join("exit-snapshots"))
        .map(|dir| dir.map(|entry| entry.unwrap().path()).collect())
        .unwrap_or_default()
}

/// The saved file, the inline tail and the closed terminal all describe one exit.
fn assert_snapshot(f: &Fixture, pane: &ExitedPane, text: &str) -> PathBuf {
    let files = snapshot_files(f);
    assert_eq!(files.len(), 1, "{files:?}");
    let file = &files[0];
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(&format!("a{}-", f.agent.0)), "{name}");
    let saved = std::fs::read_to_string(file).unwrap();
    assert!(saved.contains("visible screen top"), "{saved}");
    assert!(saved.contains("codex output 31\n"), "{saved}");
    assert!(saved.contains(LAST_LINE), "{saved}");
    assert!(text.contains(&file.display().to_string()), "{text}");
    // The last 20 non-blank lines, oldest first, ending with what ran last.
    assert!(text.contains("codex output 213"), "{text}");
    assert!(!text.contains("codex output 212\n"), "{text}");
    assert!(text.contains("codex: fatal: stream disconnected"), "{text}");
    assert!(text.trim_end().ends_with(LAST_LINE), "{text}");
    let closes = pane.closes.lock().unwrap();
    assert_eq!(closes.len(), 1, "{closes:?}");
    assert!(closes[0].close_exited_shell && closes[0].allow_busy_shell);
    file.clone()
}

#[test]
fn provider_exit_with_open_request_saves_its_screen_reports_the_tail_and_closes_the_terminal() {
    let mut f = Fixture::new();
    let orch = f.orch;
    exited_worker(&mut f);
    let message = f.send(Author::Agent(orch), "open work");
    let message = f.worker.state.request(message).unwrap().prompt.id;
    let pane = ExitedPane::install(&mut f, 0);
    f.worker.poll().unwrap();
    deliver(&mut f);
    let notice = f.calls.lock().unwrap().concat();
    assert!(notice.contains("Exited with open request"), "{notice}");
    let file = assert_snapshot(&f, &pane, &notice);
    let path = file.display().to_string();
    let status = f.call("message.status", json!({"message": message.0.to_string()}));
    let status = serde_json::to_string(&status).unwrap();
    assert!(status.contains(&path.replace('\\', "\\\\")), "{status}");
    let state = serde_json::to_string(&f.call("state", json!({}))).unwrap();
    assert!(state.contains(&path.replace('\\', "\\\\")), "{state}");
    // Later polls see the closed pane: no second snapshot, close or notice.
    f.worker.poll().unwrap();
    deliver(&mut f);
    assert_eq!(snapshot_files(&f).len(), 1);
    assert_eq!(pane.closes.lock().unwrap().len(), 1);
}

#[test]
fn provider_exit_while_idle_saves_screen_in_state_without_chat() {
    let mut f = Fixture::new();
    exited_worker(&mut f);
    let pane = ExitedPane::install(&mut f, 0);
    f.worker.poll().unwrap();
    assert!(
        f.worker
            .state
            .agent(f.agent)
            .unwrap()
            .runtime_identity
            .pane_id
            .is_none(),
        "a successfully closed exit pane must no longer be focusable"
    );
    let master = f.worker.state.master_room().unwrap().id;
    assert_eq!(f.worker.state.notices(master).count(), 0);
    let snapshot = f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .exit_snapshot
        .as_ref()
        .unwrap();
    assert_snapshot(&f, &pane, &snapshot.notice());
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn deleting_an_agent_after_its_provider_exited_leaves_no_terminal_open() {
    // 0: Bus closed the terminal at exit. 1: that close was refused, so
    // deletion closes the busy shell the exited provider left.
    for refuse_closes in [0, 1] {
        let mut f = Fixture::new();
        exited_worker(&mut f);
        let pane = ExitedPane::install(&mut f, refuse_closes);
        f.worker.poll().unwrap();
        assert_eq!(snapshot_files(&f).len(), 1);
        let (events, received) = mpsc::channel();
        f.worker
            .command(BusCommand::DeleteAgent(f.agent), &events)
            .unwrap();
        assert_eq!(f.worker.snapshot().error, None, "{refuse_closes}");
        assert!(f.worker.state.agent(f.agent).is_none(), "{refuse_closes}");
        assert!(
            !received
                .try_iter()
                .any(|event| matches!(event, BusEvent::TerminalsLeftOpen(_))),
            "{refuse_closes}"
        );
        let closes = pane.closes.lock().unwrap();
        assert!(
            closes
                .iter()
                .all(|c| c.allow_busy_shell || !c.close_exited_shell),
            "{closes:?}"
        );
        assert_eq!(
            closes.iter().filter(|c| c.allow_busy_shell).count(),
            refuse_closes + 1,
            "{closes:?}"
        );
        // A terminal Bus already closed is not closed again; a refused one
        // gets the ordinary guard, then the exited-shell close.
        assert_eq!(closes.len(), 1 + 2 * refuse_closes, "{closes:?}");
    }
}

/// The terminal closes only after the exit is durable, so when the exit
/// poll's save fails the next poll still reports the exit to the orchestrator.
#[test]
fn an_exit_whose_poll_save_fails_is_reported_on_the_next_poll() {
    let mut f = Fixture::new();
    let orch = f.orch;
    exited_worker(&mut f);
    f.send(Author::Agent(orch), "open work");
    let _pane = ExitedPane::install(&mut f, 0);
    f.worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    assert!(
        f.worker.poll().is_err(),
        "the injected save failure must fail this poll"
    );
    f.worker.poll().unwrap();
    deliver(&mut f);
    let notice = f.calls.lock().unwrap().concat();
    assert!(
        notice.contains("Exited with open request"),
        "exit notice lost: {notice:?}"
    );
}

/// One exited pane with a chosen screen; every guarded close succeeds.
struct ScreenPane {
    pane: &'static str,
    terminal: &'static str,
    screen: String,
    closed: Arc<Mutex<Vec<String>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Transport for ScreenPane {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![] }),
            Method::PaneGet(p) if p.pane_id == self.pane => Ok(ResponseResult::PaneInfo { pane: serde_json::from_value(json!({
                "pane_id":p.pane_id,"terminal_id":self.terminal,"workspace_id":"w","tab_id":"t","focused":false,"agent_status":"unknown","revision":1,"agent_process_exited":true
            })).unwrap() }),
            Method::PaneGet(_) => refused("pane_not_found"),
            Method::PaneRead(p) if p.pane_id == self.pane => Ok(ResponseResult::PaneRead { read: serde_json::from_value(json!({
                "pane_id":p.pane_id,"workspace_id":"w","tab_id":"t","source":p.source,"format":"text","text":self.screen,"revision":1,"truncated":false
            })).unwrap() }),
            Method::PaneCloseIfIdentity(p) => {
                self.closed.lock().unwrap().push(p.pane_id);
                Ok(ResponseResult::Ok {})
            }
            other => Accepting(self.calls.clone()).request(other),
        }
    }
}

fn install_screen(
    f: &mut Fixture,
    pane: &'static str,
    terminal: &'static str,
    screen: &str,
) -> Arc<Mutex<Vec<String>>> {
    let closed = Arc::new(Mutex::new(Vec::new()));
    f.worker.transport = Box::new(ScreenPane {
        pane,
        terminal,
        screen: screen.into(),
        closed: closed.clone(),
        calls: f.calls.clone(),
    });
    closed
}

/// The exit snapshot and close are for workers; the MASTER orchestrator's
/// own terminal is the developer's and stays open when its provider exits.
#[test]
fn an_orchestrator_provider_exit_is_saved_but_leaves_its_terminal_open() {
    let mut f = Fixture::new();
    let closed = install_screen(&mut f, "pane", "terminal", "orchestrator screen");
    f.worker.poll().unwrap();
    let closed = closed.lock().unwrap();
    assert!(
        !closed.iter().any(|pane| pane == "pane"),
        "orchestrator terminal closed: {closed:?}"
    );
    let snapshot = f.worker.state.agent(f.orch).unwrap().exit_snapshot.clone();
    assert!(
        snapshot
            .as_ref()
            .is_some_and(|s| s.path.is_some() && s.terminal_closed == Some(false)),
        "{snapshot:?}"
    );
}

/// Saved exit output remains inspectable without paging the developer.
#[test]
fn saved_exit_output_pages_no_one() {
    let mut f = Fixture::new();
    exited_worker(&mut f);
    install_screen(
        &mut f,
        "worker-pane",
        "worker-terminal",
        "build log\nWAITING ON YOU: paste the prod token here",
    );
    let before = f.worker.state.clone();
    f.worker.poll().unwrap();
    let master = f.worker.state.master_room().unwrap().id;
    assert_eq!(f.worker.state.notices(master).count(), 0);
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .exit_snapshot
        .as_ref()
        .unwrap()
        .tail
        .contains("paste the prod token"));
    let mut waiting = crate::messaging::feishu::waiting::WaitingNotifications::default();
    waiting.configure(true);
    let now = io::now_ms();
    waiting.observe(&before, &f.worker.state, now);
    let sent = waiting.take(now + 10_000);
    assert_eq!(sent, None, "screen text paged the developer");
}

/// A request sent to a worker after its provider exited and Bus closed its
/// terminal still tells the orchestrator the worker exited.
#[test]
fn a_request_sent_after_bus_closed_the_exited_terminal_reports_the_exit() {
    let mut f = Fixture::new();
    let orch = f.orch;
    exited_worker(&mut f);
    let pane = ExitedPane::install(&mut f, 0);
    f.worker.poll().unwrap();
    assert_eq!(pane.closes.lock().unwrap().len(), 1);
    f.send(Author::Agent(orch), "work sent after the exit");
    f.worker.poll().unwrap();
    deliver(&mut f);
    let notice = f.calls.lock().unwrap().concat();
    assert!(
        notice.contains("Exited with open request"),
        "no exit notice for the later request: {notice:?}"
    );
}

/// Control: while the exited terminal stays open, a later request gets the exit notice.
#[test]
fn a_request_sent_after_the_exit_reports_it_while_the_terminal_stays_open() {
    let mut f = Fixture::new();
    let orch = f.orch;
    exited_worker(&mut f);
    let _pane = ExitedPane::install(&mut f, usize::MAX);
    f.worker.poll().unwrap();
    f.send(Author::Agent(orch), "work sent after the exit");
    f.worker.poll().unwrap();
    deliver(&mut f);
    let notice = f.calls.lock().unwrap().concat();
    assert!(notice.contains("Exited with open request"), "{notice:?}");
}

struct WideExitScreen(ExitedPane);

impl Transport for WideExitScreen {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if let Method::PaneRead(params) = &method {
            if params.source == schema::ReadSource::Recent {
                let text = (1..=20)
                    .map(|line| format!("fatal diagnostic {line:02}: {}", "x".repeat(159)))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(ResponseResult::PaneRead {
                    read: serde_json::from_value(json!({
                        "pane_id":params.pane_id,"workspace_id":"w","tab_id":"t",
                        "source":params.source,"format":"text","text":text,
                        "revision":1,"truncated":false
                    }))
                    .unwrap(),
                });
            }
        }
        self.0.request(method)
    }
}

#[test]
fn a_wide_exit_screen_still_delivers_the_exit_and_snapshot_path() {
    let mut f = Fixture::new();
    exited_worker(&mut f);
    let orch = f.orch;
    f.send(Author::Agent(orch), "open work");
    let pane = ExitedPane::install(&mut f, 0);
    f.worker.transport = Box::new(WideExitScreen(ExitedPane {
        calls: pane.calls.clone(),
        closes: pane.closes.clone(),
        refuse_closes: 0,
        open: true,
    }));

    f.worker.poll().unwrap();
    let snapshot = f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .exit_snapshot
        .clone()
        .unwrap();
    assert_eq!(snapshot.terminal_closed, Some(true));
    assert!(snapshot.tail.contains("fatal diagnostic 20"));
    deliver(&mut f);

    let delivered = f.calls.lock().unwrap().concat();
    assert!(
        delivered.contains("Exited with open request")
            && delivered.contains(&snapshot.path.unwrap().display().to_string()),
        "a 180-column exit screen must still deliver the exit and snapshot path; got {delivered:?}",
    );
}

#[test]
fn the_exit_snapshot_and_notice_survive_a_transient_state_save_failure() {
    let mut f = Fixture::new();
    exited_worker(&mut f);
    let orch = f.orch;
    let request = f.send(Author::Agent(orch), "open work");
    let pane = ExitedPane::install(&mut f, 0);
    f.worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempSync);

    assert!(f.worker.poll().is_err());
    let files = snapshot_files(&f);
    assert_eq!(
        files.len(),
        1,
        "the screen file was saved before state failed"
    );

    let (events, _received) = mpsc::channel();
    f.worker
        .retry_storage(std::time::Instant::now() + Duration::from_secs(2), &events);
    assert!(f.worker.storage_pause.is_none());
    for _ in 0..2 {
        f.worker.poll().unwrap();
    }
    deliver(&mut f);

    let status = serde_json::to_string(&f.call(
        "message.status",
        json!({"message": f.worker.state.request(request).unwrap().prompt.id.0.to_string()}),
    ))
    .unwrap();
    let snapshot = f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .exit_snapshot
        .as_ref();
    // The failed save left the pane open, so the next poll saved its screen
    // again; the recorded file is one of those captures.
    let saved = snapshot.and_then(|saved| saved.path.clone());
    assert!(
        saved.as_ref().is_some_and(|path| snapshot_files(&f).contains(path)),
        "storage recovered but the exit snapshot was lost: snapshot={snapshot:?}, close_count={}, message_status={status}, delivered={:?}",
        pane.closes.lock().unwrap().len(),
        f.calls.lock().unwrap(),
    );
    assert!(f.calls.lock().unwrap().iter().any(|text| {
        text.contains("Exited with open request")
            && text.contains(&saved.as_ref().unwrap().display().to_string())
            && text.contains(LAST_LINE)
    }));
}

#[cfg(unix)]
#[path = "exit_snapshot_save_retry_test.rs"]
mod save_retry;
