use super::*;
use crate::protocol::api::schema::PaneCloseIfIdentityParams;
use std::sync::{Arc, Mutex};

const SAVED_TERMINAL: &str = "terminal";
const SAVED_PANE: &str = "pane";
const REPLACEMENT_TERMINAL: &str = "term-restart";

/// What resume observes once it has attempted the guarded close.
#[derive(Clone, Copy)]
enum PaneAfterClose {
    /// The saved terminal still matches and the guarded close succeeds.
    SameAndClosed,
    /// A whole-session restart recreated this pane on a new terminal.
    Replaced,
    /// The saved terminal still matches and still hosts the provider.
    StillHosting,
}

struct ResumeClose {
    mode: PaneAfterClose,
    closes: Arc<Mutex<Vec<PaneCloseIfIdentityParams>>>,
    starts: Arc<Mutex<u32>>,
    /// Poll reads the pane before resume closes it. That read still reports the
    /// saved terminal; otherwise poll detaches the pane and resume never reaches
    /// the close guard. After the guarded close is attempted, a replaced pane
    /// reports its new terminal id.
    close_attempted: bool,
}

impl ResumeClose {
    fn pane(pane_id: &str, terminal_id: &str, hosts_provider: bool) -> schema::PaneInfo {
        serde_json::from_value(json!({
            "pane_id": pane_id,
            "terminal_id": terminal_id,
            "workspace_id": "w1",
            "tab_id": "t1",
            "focused": false,
            "agent_status": "unknown",
            "revision": 1,
            "agent_process_exited": false,
            "agent": hosts_provider.then_some("codex"),
        }))
        .unwrap()
    }

    fn identity_changed() -> TransportError {
        TransportError {
            timestamps: Default::default(),
            code: Some("terminal_identity_changed".into()),
            message: "Terminal or provider session changed; pane was not closed".into(),
            definitely_rejected: true,
        }
    }
}

impl Transport for ResumeClose {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                Ok(ResponseResult::AgentList { agents: vec![] })
            }
            Method::PaneGet(target) => {
                let replaced =
                    self.close_attempted && matches!(self.mode, PaneAfterClose::Replaced);
                let terminal = if replaced {
                    REPLACEMENT_TERMINAL
                } else {
                    SAVED_TERMINAL
                };
                let hosts_provider = matches!(self.mode, PaneAfterClose::StillHosting) && !replaced;
                Ok(ResponseResult::PaneInfo {
                    pane: Self::pane(&target.pane_id, terminal, hosts_provider),
                })
            }
            Method::PaneCloseIfIdentity(params) => {
                self.closes.lock().unwrap().push(params);
                self.close_attempted = true;
                match self.mode {
                    PaneAfterClose::SameAndClosed => Ok(ResponseResult::Ok {}),
                    PaneAfterClose::Replaced | PaneAfterClose::StillHosting => {
                        Err(Self::identity_changed())
                    }
                }
            }
            Method::TabCreate(_) => Ok(ResponseResult::TabCreated {
                tab: serde_json::from_value(json!({
                    "tab_id": "t2",
                    "workspace_id": "w1",
                    "number": 2,
                    "label": "resume",
                    "focused": false,
                    "pane_count": 1,
                    "agent_status": "unknown"
                }))
                .unwrap(),
                root_pane: serde_json::from_value(json!({
                    "pane_id": "pane-new",
                    "terminal_id": "term-new",
                    "workspace_id": "w1",
                    "tab_id": "t2",
                    "focused": false,
                    "agent_status": "unknown",
                    "revision": 1
                }))
                .unwrap(),
            }),
            Method::AgentStart(_) | Method::AgentStartIfIdentity(_) => {
                *self.starts.lock().unwrap() += 1;
                let mut agent = native_info("pane-new", "codex", "session");
                agent.terminal_id = "term-new".into();
                Ok(ResponseResult::AgentStarted {
                    agent,
                    argv: vec!["codex".into(), "resume".into(), "session".into()],
                })
            }
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

fn arm_unavailable_resume(worker: &mut Worker, agent: AgentId) {
    let hooks = worker.data_dir.join(".codex/hooks.json");
    let document = crate::agents::providers::hook_json::merge_hooks(
        json!({}),
        crate::agents::providers::hook_json::HookContract::for_provider(Provider::Codex),
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    std::fs::write(hooks, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut state = worker.state.clone();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 1)
        .unwrap();
    worker.save(state).unwrap();
    worker
        .missing_binary_checked
        .insert(agent, Some(SAVED_TERMINAL.into()));
}

struct ResumeObs {
    result: serde_json::Value,
    closes: Vec<PaneCloseIfIdentityParams>,
    starts: u32,
    worker: Worker,
    dir: PathBuf,
}

fn resume_with(mode: PaneAfterClose) -> ResumeObs {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    arm_unavailable_resume(&mut worker, agent);
    let closes = Arc::new(Mutex::new(Vec::new()));
    let starts = Arc::new(Mutex::new(0));
    worker.transport = Box::new(ResumeClose {
        mode,
        closes: Arc::clone(&closes),
        starts: Arc::clone(&starts),
        close_attempted: false,
    });
    let result = worker.resume_room(room).unwrap();
    let closes = closes.lock().unwrap().clone();
    let starts = *starts.lock().unwrap();
    ResumeObs {
        result,
        closes,
        starts,
        worker,
        dir,
    }
}

fn saved_close(closes: &[PaneCloseIfIdentityParams]) -> &PaneCloseIfIdentityParams {
    let close = closes
        .iter()
        .find(|close| close.expected_terminal_id == SAVED_TERMINAL)
        .expect("resume closes the saved terminal");
    assert_eq!(close.pane_id, SAVED_PANE);
    assert_eq!(close.expected_agent, "codex");
    assert_eq!(close.expected_session_id.as_deref(), Some("session"));
    assert!(close.close_exited_shell);
    assert!(!close.allow_busy_shell);
    close
}

fn agent_row(obs: &ResumeObs) -> &serde_json::Value {
    &obs.result["agents"][0]
}

#[test]
fn resume_closes_the_saved_terminal_when_its_identity_still_matches() {
    let obs = resume_with(PaneAfterClose::SameAndClosed);
    let row = agent_row(&obs);
    assert_eq!(
        row["outcome"], "resuming",
        "a matching saved terminal still has to be closed and verified: {row}"
    );
    let close = saved_close(&obs.closes);
    assert_eq!(
        obs.closes.len(),
        1,
        "only the saved terminal is closed: {close:?}"
    );
    assert_eq!(
        obs.starts, 1,
        "resume starts the provider after the close is verified"
    );
    let snapshot = obs
        .worker
        .state
        .agents()
        .find(|agent| agent.runtime_identity.session_id.as_deref() == Some("session"))
        .and_then(|agent| agent.exit_snapshot.clone());
    assert!(
        snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.terminal_closed != Some(true)),
        "a verified close does not invent a restarted-terminal snapshot: {snapshot:?}"
    );
    drop(obs.worker);
    std::fs::remove_dir_all(obs.dir).unwrap();
}

#[test]
fn resume_records_the_saved_terminal_closed_when_restart_recreated_the_pane() {
    let obs = resume_with(PaneAfterClose::Replaced);
    let row = agent_row(&obs);
    assert_eq!(
        row["outcome"], "resuming",
        "a replaced terminal must not strand resume: {row}"
    );
    let close = saved_close(&obs.closes);
    assert!(
        obs.closes
            .iter()
            .all(|close| close.expected_terminal_id == SAVED_TERMINAL),
        "resume must not close the replacement pane: {close:?} all={:?}",
        obs.closes
    );
    assert_eq!(
        obs.starts, 1,
        "resume starts the provider after recording the saved terminal closed"
    );
    let agent = obs
        .worker
        .state
        .agents()
        .find(|agent| agent.runtime_identity.session_id.as_deref() == Some("session"))
        .expect("resumed agent");
    let snapshot = agent
        .exit_snapshot
        .as_ref()
        .expect("saved terminal recorded closed");
    assert_eq!(snapshot.terminal_id, SAVED_TERMINAL);
    assert_eq!(snapshot.terminal_closed, Some(true));
    drop(obs.worker);
    std::fs::remove_dir_all(obs.dir).unwrap();
}

#[test]
fn resume_blocks_when_the_saved_terminal_still_hosts_the_provider() {
    let obs = resume_with(PaneAfterClose::StillHosting);
    let row = agent_row(&obs);
    assert_eq!(
        row["outcome"], "blocked",
        "a live provider pane must block resume: {row}"
    );
    let reason = row["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("Previous pane could not be safely closed"),
        "the strict close failure stays visible: {reason}"
    );
    let _close = saved_close(&obs.closes);
    assert_eq!(
        obs.starts, 0,
        "no provider starts while its pane is unconfirmed"
    );
    let snapshot = obs
        .worker
        .state
        .agents()
        .find(|agent| agent.runtime_identity.session_id.as_deref() == Some("session"))
        .and_then(|agent| agent.exit_snapshot.clone());
    assert!(
        snapshot.as_ref().is_none_or(|snapshot| {
            snapshot.terminal_id != SAVED_TERMINAL || snapshot.terminal_closed != Some(true)
        }),
        "a refused close must not record the live terminal closed: {snapshot:?}"
    );
    drop(obs.worker);
    std::fs::remove_dir_all(obs.dir).unwrap();
}
