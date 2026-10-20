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
                    return refused("pane_not_found");
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
fn provider_exit_while_idle_saves_its_screen_in_the_exit_notice_and_closes_the_terminal() {
    let mut f = Fixture::new();
    exited_worker(&mut f);
    let pane = ExitedPane::install(&mut f, 0);
    f.worker.poll().unwrap();
    let master = f.worker.state.master_room().unwrap().id;
    let notice = f
        .worker
        .state
        .notices(master)
        .map(|p| p.text.clone())
        .find(|text| text.contains("Agent process exited"))
        .expect("exit notice");
    assert_snapshot(&f, &pane, &notice);
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
