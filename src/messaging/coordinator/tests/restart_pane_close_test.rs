use super::*;
use crate::protocol::api::schema::PaneCloseIfIdentityParams;
use std::sync::{Arc, Mutex};

const SAVED_TERMINAL: &str = "terminal";
const SAVED_PANE: &str = "pane";
const REPLACEMENT_TERMINAL: &str = "term-restart";

/// Server rules for `pane.close_if_identity`, with the pane terminal fixed
/// before resume starts. A dead session's terminal is absent, so closing that
/// id succeeds and does not touch the replacement pane. A registered terminal
/// that is not the pane's current terminal is refused. A pane that still hosts
/// the provider is refused even when the terminal id matches, because
/// `close_exited_shell` does not own a live provider.
struct RelaunchServer {
    current_terminal: &'static str,
    saved_terminal_alive: bool,
    hosts_provider: bool,
    /// Reported only after the guarded close returns. Poll still sees
    /// `current_terminal`, so this does not model a pane the relaunched TUI
    /// had already recreated.
    terminal_after_close: Option<&'static str>,
    close_attempted: bool,
    closes: Arc<Mutex<Vec<PaneCloseIfIdentityParams>>>,
    starts: Arc<Mutex<u32>>,
}

impl RelaunchServer {
    fn pane(&self) -> schema::PaneInfo {
        let terminal = if self.close_attempted {
            self.terminal_after_close.unwrap_or(self.current_terminal)
        } else {
            self.current_terminal
        };
        let mut value = json!({
            "pane_id": SAVED_PANE,
            "terminal_id": terminal,
            "workspace_id": "w1",
            "tab_id": "t1",
            "focused": false,
            "agent_status": "unknown",
            "revision": 1,
            "agent_process_exited": false,
        });
        if self.hosts_provider {
            value["agent"] = json!("codex");
            value["agent_session"] = json!({
                "source": "herdr:codex",
                "agent": "codex",
                "kind": "id",
                "value": "session"
            });
        }
        serde_json::from_value(value).unwrap()
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

impl Transport for RelaunchServer {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => {
                Ok(ResponseResult::AgentList { agents: vec![] })
            }
            Method::PaneGet(_) => Ok(ResponseResult::PaneInfo { pane: self.pane() }),
            Method::PaneCloseIfIdentity(params) => {
                self.closes.lock().unwrap().push(params.clone());
                self.close_attempted = true;
                if params.expected_terminal_id == SAVED_TERMINAL && !self.saved_terminal_alive {
                    return Ok(ResponseResult::Ok {});
                }
                if params.expected_terminal_id != self.current_terminal
                    || params.pane_id != SAVED_PANE
                {
                    return Err(Self::identity_changed());
                }
                if self.hosts_provider {
                    return Err(Self::identity_changed());
                }
                Ok(ResponseResult::Ok {})
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

fn resume_relaunched(
    current_terminal: &'static str,
    saved_terminal_alive: bool,
    hosts_provider: bool,
    terminal_after_close: Option<&'static str>,
) -> ResumeObs {
    let _providers = provider_binaries();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    arm_unavailable_resume(&mut worker, agent);
    let closes = Arc::new(Mutex::new(Vec::new()));
    let starts = Arc::new(Mutex::new(0));
    worker.transport = Box::new(RelaunchServer {
        current_terminal,
        saved_terminal_alive,
        hosts_provider,
        terminal_after_close,
        close_attempted: false,
        closes: Arc::clone(&closes),
        starts: Arc::clone(&starts),
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

fn agent_row(obs: &ResumeObs) -> &serde_json::Value {
    &obs.result["agents"][0]
}

fn saved_terminal_marked_closed(obs: &ResumeObs) -> bool {
    obs.worker.state.agents().any(|agent| {
        agent.exit_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.terminal_id == SAVED_TERMINAL && snapshot.terminal_closed == Some(true)
        })
    })
}

fn assert_saved_close(obs: &ResumeObs) {
    assert_eq!(obs.closes.len(), 1, "the guard ran once: {:?}", obs.closes);
    let close = &obs.closes[0];
    assert_eq!(close.pane_id, SAVED_PANE);
    assert_eq!(close.expected_terminal_id, SAVED_TERMINAL);
    assert_eq!(close.expected_agent, "codex");
    assert_eq!(close.expected_session_id.as_deref(), Some("session"));
    assert!(close.close_exited_shell);
    assert!(!close.allow_busy_shell);
}

fn finish(obs: ResumeObs) {
    drop(obs.worker);
    std::fs::remove_dir_all(obs.dir).unwrap();
}

#[test]
fn resume_closes_the_saved_terminal_when_its_identity_still_matches() {
    let obs = resume_relaunched(SAVED_TERMINAL, true, false, None);
    let row = agent_row(&obs);
    assert_eq!(
        row["outcome"], "resuming",
        "a matching saved terminal still has to be closed and verified: {row}"
    );
    assert_saved_close(&obs);
    assert_eq!(
        obs.starts, 1,
        "resume starts the provider after the close is verified"
    );
    assert!(
        !saved_terminal_marked_closed(&obs),
        "a verified close does not invent a restarted-terminal snapshot"
    );
    finish(obs);
}

/// Measured on base `9b219860e`: the first PaneGet already shows the new
/// terminal, poll clears the saved pane, and `close_resume_terminal` returns
/// before the guard. The close-guard error is not produced.
#[test]
fn dead_session_terminal_relaunch_does_not_strand_on_the_pane_close_guard() {
    let obs = resume_relaunched(REPLACEMENT_TERMINAL, false, false, None);
    assert_relaunch_resumes_without_reaching_the_guard(&obs);
    finish(obs);
}

/// The saved terminal is still registered, and the pane already has the new
/// id before resume. Poll still clears that pane on the first read, so the
/// guard never sees the mismatch.
#[test]
fn recreated_pane_with_the_saved_terminal_still_registered_does_not_strand_on_the_pane_close_guard()
{
    let obs = resume_relaunched(REPLACEMENT_TERMINAL, true, false, None);
    assert_relaunch_resumes_without_reaching_the_guard(&obs);
    finish(obs);
}

fn assert_relaunch_resumes_without_reaching_the_guard(obs: &ResumeObs) {
    let row = agent_row(obs);
    assert_eq!(row["outcome"], "resuming", "{row}");
    let reason = row["reason"].as_str().unwrap_or("");
    assert!(
        !reason.contains("Previous pane could not be safely closed"),
        "{row}"
    );
    assert!(
        obs.closes.is_empty(),
        "poll detaches the recreated pane before the guard: {:?}",
        obs.closes
    );
    assert_eq!(obs.starts, 1);
    assert!(!saved_terminal_marked_closed(obs));
    let agent = obs
        .worker
        .state
        .agents()
        .find(|agent| agent.runtime_identity.session_id.as_deref() == Some("session"))
        .expect("resumed agent");
    assert_eq!(agent.runtime_identity.pane_id.as_deref(), Some("pane-new"));
    assert_eq!(
        agent.runtime_identity.terminal_id.as_deref(),
        Some("term-new")
    );
}

#[test]
fn live_provider_on_its_saved_terminal_blocks_when_close_is_unconfirmed() {
    let obs = resume_relaunched(SAVED_TERMINAL, true, true, None);
    assert_unconfirmed_host_blocks(&obs);
    finish(obs);
}

/// The pane hosts the provider when the guard runs. A later read that shows a
/// different terminal, still hosting that provider, must not turn the refusal
/// into a resume.
#[test]
fn unconfirmed_close_keeps_blocking_when_a_later_read_still_hosts_the_provider() {
    let obs = resume_relaunched(SAVED_TERMINAL, true, true, Some(REPLACEMENT_TERMINAL));
    assert_unconfirmed_host_blocks(&obs);
    finish(obs);
}

fn assert_unconfirmed_host_blocks(obs: &ResumeObs) {
    let row = agent_row(obs);
    assert_eq!(
        row["outcome"], "blocked",
        "a pane hosting the provider must block resume: {row}"
    );
    let reason = row["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("Previous pane could not be safely closed"),
        "the unconfirmed close stays visible: {reason}"
    );
    assert!(
        reason.contains("Terminal or provider session changed; pane was not closed"),
        "the refusal is the ownership failure: {reason}"
    );
    assert_saved_close(obs);
    assert_eq!(
        obs.starts, 0,
        "no provider starts while its pane is unconfirmed"
    );
    assert!(
        !saved_terminal_marked_closed(obs),
        "an unconfirmed close must not record the live terminal closed"
    );
}
