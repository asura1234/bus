use super::*;
use crate::messaging::native::TransportError;
use std::sync::{Arc, Mutex};

const CAPTURE: &str =
    include_str!("../../../../tests/fixtures/provider-ui/claude-extra-thought.txt");
const SHAPE: &str = "6105abde0622e6a34d10c0743d1ad884f13286da890fb7882cf0618c7fbf83a8";

fn parsed(screen: &str) -> schema::AgentDialog {
    let dialog = crate::agents::dialog::parse(screen).unwrap();
    schema::AgentDialog {
        id: dialog.id(),
        digest: dialog.digest(),
        text: dialog.text.clone(),
        kind: match dialog.kind {
            crate::agents::dialog::DialogKind::Choice => schema::AgentDialogKind::Choice,
            crate::agents::dialog::DialogKind::Question => schema::AgentDialogKind::Question,
        },
        options: dialog
            .options
            .iter()
            .map(|option| schema::AgentDialogOption {
                number: option.number,
                label: option.label.clone(),
                selected: option.selected,
                free_text: option.free_text,
            })
            .collect(),
        free_text: dialog.free_text_summary().map(|(option, draft, checkbox)| {
            schema::AgentDialogFreeText {
                option,
                draft,
                checkbox,
            }
        }),
        tabs: Vec::new(),
        hint: dialog.hint,
    }
}

struct Native {
    info: schema::AgentInfo,
    dialog: Option<schema::AgentDialog>,
    actions: Vec<u32>,
    focuses: Vec<u32>,
    keep_open: bool,
    uncertain: bool,
    reads: usize,
    attempts: usize,
    replace_on_read: Option<(usize, schema::AgentDialog)>,
    replace_on_choose: Option<schema::AgentDialog>,
    refuse_once: bool,
    pending_question: bool,
    observation_unavailable: bool,
    state_file: PathBuf,
    agent: AgentId,
    durable_reservation_seen: bool,
}

struct AutoServer(Arc<Mutex<Native>>);
impl Transport for AutoServer {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let mut native = self.0.lock().unwrap();
        let observation = |native: &Native, pane: String| schema::AgentDialogObservation {
            pending_question: native.pending_question,
            terminal_id: "terminal".into(),
            pane_id: pane,
            session_id: Some("session".into()),
            content_revision: 10,
            dialog: native.dialog.clone(),
        };
        match method {
            Method::AgentList(_) => {
                let mut info = native.info.clone();
                info.dialog_id = native.dialog.as_ref().map(|d| d.id.clone());
                info.agent_status = if native.dialog.is_some() {
                    schema::AgentStatus::Blocked
                } else {
                    schema::AgentStatus::Working
                };
                Ok(ResponseResult::AgentList { agents: vec![info] })
            }
            Method::AgentDialogObserve(params) => {
                if native.observation_unavailable {
                    return Err(TransportError {
                        code: None,
                        message: "dialog observation unavailable".into(),
                        definitely_rejected: true,
                    });
                }
                native.reads += 1;
                if native
                    .replace_on_read
                    .as_ref()
                    .is_some_and(|(at, _)| *at == native.reads)
                {
                    let (_, replacement) = native.replace_on_read.take().unwrap();
                    native.dialog = Some(replacement);
                }
                Ok(ResponseResult::AgentDialog {
                    observation: observation(&native, params.target),
                })
            }
            Method::AgentDialogChoose(params) => {
                native.attempts += 1;
                if let Some(replacement) = native.replace_on_choose.take() {
                    native.dialog = Some(replacement);
                }
                let mut matches = native
                    .dialog
                    .as_ref()
                    .is_some_and(|d| d.digest == params.expected_dialog_digest);
                if native.refuse_once {
                    matches = false;
                    native.refuse_once = false;
                }
                if matches {
                    let saved: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&native.state_file).unwrap())
                            .unwrap();
                    native.durable_reservation_seen = saved["state"]["agents"]
                        [native.agent.0.to_string()]["automatic_dialog_answer"]
                        .is_string();
                    if params.action == schema::AgentDialogChoiceAction::Focus {
                        native.focuses.push(params.option);
                        let dialog = native.dialog.as_mut().unwrap();
                        for option in &mut dialog.options {
                            option.selected = option.number == params.option;
                        }
                        dialog.digest.push_str("-focused");
                    } else {
                        assert_ne!(
                            params.action,
                            schema::AgentDialogChoiceAction::Choose,
                            "automatic replies must never use delayed confirmation"
                        );
                        assert!(native
                            .dialog
                            .as_ref()
                            .unwrap()
                            .options
                            .iter()
                            .any(|row| row.number == params.option && row.selected));
                        native.actions.push(params.option);
                    }
                    if native.uncertain && params.action == schema::AgentDialogChoiceAction::Confirm
                    {
                        return Err(TransportError {
                            code: None,
                            message: "lost native response".into(),
                            definitely_rejected: false,
                        });
                    }
                    if !native.keep_open
                        && params.action == schema::AgentDialogChoiceAction::Confirm
                    {
                        native.dialog = None;
                    }
                }
                Ok(ResponseResult::AgentDialogChosen {
                    choice: schema::AgentDialogChooseResult {
                        written: matches,
                        reason: (!matches).then(|| "stale_or_changed_dialog".into()),
                        keys: if matches {
                            vec![if params.action == schema::AgentDialogChoiceAction::Focus {
                                "down"
                            } else {
                                "enter"
                            }
                            .into()]
                        } else {
                            Vec::new()
                        },
                        observation: observation(&native, params.target),
                    },
                })
            }
            other => panic!("unexpected native request in automatic UI test: {other:?}"),
        }
    }
}

struct Rig {
    worker: Worker,
    agent: AgentId,
    room: RoomId,
    orchestrator: AgentId,
    native: Arc<Mutex<Native>>,
    dir: PathBuf,
}
impl Rig {
    fn new() -> Self {
        Self::with_provider(Provider::ClaudeCode)
    }
    fn with_provider(provider: Provider) -> Self {
        let kind = match provider {
            Provider::ClaudeCode => "claude",
            Provider::Codex => "codex",
            Provider::Cursor => "cursor",
            Provider::Grok => "grok",
            Provider::Kimi => "kimi",
            #[cfg(feature = "dev-tools")]
            Provider::Fake => "fake",
        };
        let dir = crate::utils::test_temp::unique_temp_path("bus-auto-ui");
        let native = Arc::new(Mutex::new(Native {
            info: serde_json::from_value(serde_json::json!({
                "terminal_id":"terminal", "pane_id":"pane", "name":"builder", "agent":kind,
                "workspace_id":"w", "tab_id":"t", "focused":false, "interactive_ready":true,
                "revision":1, "agent_status":"blocked",
                "agent_session":{"source":format!("herdr:{kind}"),"agent":kind,"kind":"id","value":"session"}
            }))
            .unwrap(),
            dialog: Some(parsed(CAPTURE)),
            actions: Vec::new(), focuses: Vec::new(),
            keep_open: false,
            uncertain: false,
            reads: 0, attempts: 0, replace_on_read: None, replace_on_choose: None, refuse_once: false,
            pending_question: false, observation_unavailable: false,
            state_file: dir.join("state.json"), agent: AgentId(0), durable_reservation_seen: false,
        }));
        let mut worker = Worker::open(dir.clone(), Box::new(AutoServer(native.clone()))).unwrap();
        let mut state = worker.state.clone();
        let room = state.create_room("work").unwrap();
        let agent = state
            .create_agent(room, "builder", provider, dir.clone(), None)
            .unwrap();
        state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some("launch".into()),
                    terminal_id: Some("terminal".into()),
                    pane_id: Some("pane".into()),
                    session_id: Some("session".into()),
                },
            )
            .unwrap();
        let master = state.master_room().unwrap().id;
        let orchestrator = state
            .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
            .unwrap();
        state.bind_orchestrator(orchestrator, room).unwrap();
        worker.save(state).unwrap();
        native.lock().unwrap().agent = agent;
        Self {
            worker,
            agent,
            room,
            orchestrator,
            native,
            dir,
        }
    }
    fn polls(&mut self, count: usize) {
        for _ in 0..count {
            self.worker.poll().unwrap();
        }
    }
    fn in_master(mut self) -> Self {
        let mut state = self.worker.state.clone();
        let identity = state.agent(self.agent).unwrap().runtime_identity.clone();
        state
            .set_agent_runtime_identity(self.agent, AgentRuntimeIdentity::default())
            .unwrap();
        state
            .set_agent_runtime_identity(self.orchestrator, identity)
            .unwrap();
        self.agent = self.orchestrator;
        self.room = state.master_room().unwrap().id;
        self.native.lock().unwrap().agent = self.agent;
        self.worker.save(state).unwrap();
        self
    }
    fn reopen(self) -> Self {
        let Self {
            worker,
            agent,
            room,
            orchestrator,
            native,
            dir,
        } = self;
        drop(worker);
        let worker = Worker::open(dir.clone(), Box::new(AutoServer(native.clone()))).unwrap();
        Self {
            worker,
            agent,
            room,
            orchestrator,
            native,
            dir,
        }
    }
    fn finish(self) {
        let dir = self.dir.clone();
        drop(self);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn auto_ui_fixture_matches_the_independently_recorded_native_shape() {
    assert_eq!(parsed(CAPTURE).id, SHAPE);
}

#[test]
fn auto_ui_known_shape_answers_without_waking_orchestrator() {
    const CHILD: &str = "AUTO_UI_LOG_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["messaging::coordinator::dialogs::auto_ui_tests::auto_ui_known_shape_answers_without_waking_orchestrator", "--exact", "--nocapture"])
            .env(CHILD,"1").output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed;")),
            "logging child must run exactly one test"
        );
        return;
    }
    let mut rig = Rig::new();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| rig.polls(4));
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    assert!(
        rig.native.lock().unwrap().durable_reservation_seen,
        "reservation must precede native keys"
    );
    assert!(rig
        .worker
        .state
        .queued_requests(rig.orchestrator)
        .is_empty());
    assert!(rig.worker.state.room(rig.room).unwrap().notices.is_empty());
    assert!(capture.text().contains("bus.dialog.auto_answer"));
    assert!(capture.text().contains("claude_extra_thought"));
    rig.finish();
}

#[test]
fn auto_ui_uncertain_attempt_is_not_replayed_after_restart() {
    let mut rig = Rig::new();
    {
        let mut native = rig.native.lock().unwrap();
        native.uncertain = true;
        native.keep_open = true;
    }
    rig.polls(4);
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    let mut rig = rig.reopen();
    rig.polls(4);
    assert_eq!(rig.native.lock().unwrap().actions, [2]);
    assert_eq!(rig.worker.state.queued_requests(rig.orchestrator).len(), 1);
    assert!(rig.worker.state.room(rig.room).unwrap().notices.is_empty());
    rig.finish();
}

#[path = "auto_ui_safety_test.rs"]
mod safety;
