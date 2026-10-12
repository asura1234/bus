use super::*;
use crate::messaging::native::TransportError;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What the fake server shows for the watched agent's screen.
#[derive(Clone, Default)]
struct Screen {
    dialog: bool,
    blocked: bool,
    question: bool,
    text: Option<String>,
    next_dialog: Option<(bool, String)>,
    revision: u64,
}

struct FakeServer(Arc<Mutex<Screen>>);

fn dialog() -> schema::AgentDialog {
    schema::AgentDialog {
        kind: schema::AgentDialogKind::Choice,
        text: "Do you want to proceed?".into(),
        options: vec![
            schema::AgentDialogOption {
                number: 1,
                label: "Yes".into(),
                selected: true,
            },
            schema::AgentDialogOption {
                number: 2,
                label: "No".into(),
                selected: false,
            },
        ],
        hint: Some("Esc to cancel".into()),
        id: "dialog-id".into(),
        digest: "dialog-digest".into(),
    }
}

impl Screen {
    fn current_dialog(&self) -> Option<schema::AgentDialog> {
        self.dialog.then(|| {
            let mut dialog = dialog();
            if self.question {
                dialog.kind = schema::AgentDialogKind::Question;
                dialog.text = "What token should Bus use?".into();
                dialog.options.clear();
            }
            if let Some(text) = &self.text {
                dialog.text.clone_from(text);
            }
            // Native dialog ids also change with kind/text, not selection or redraws.
            dialog.id = format!("{:?}:{}", dialog.kind, dialog.text);
            dialog.digest.clone_from(&dialog.id);
            dialog
        })
    }
}

impl Transport for FakeServer {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let screen = self.0.lock().unwrap().clone();
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList {
                agents: vec![serde_json::from_value(json!({
                    "terminal_id": "terminal", "pane_id": "pane", "name": "bus-r1-a2",
                    "agent": "claude", "workspace_id": "w", "tab_id": "t", "focused": false,
                    "interactive_ready": true, "revision": 1,
                    "agent_status": if screen.dialog || screen.blocked { "blocked" } else { "idle" },
                    "dialog_id": screen.current_dialog().map(|dialog| dialog.id),
                }))
                .unwrap()],
            }),
            Method::AgentDialogObserve(_) => Ok(ResponseResult::AgentDialog {
                observation: schema::AgentDialogObservation {
                    pending_question: false,
                    terminal_id: "terminal".into(),
                    pane_id: "pane".into(),
                    session_id: None,
                    content_revision: 4 + screen.revision,
                    dialog: screen.current_dialog(),
                },
            }),
            Method::AgentDialogChoose(_) | Method::AgentDialogAnswer(_) => {
                let mut screen = self.0.lock().unwrap();
                if let Some((question, text)) = screen.next_dialog.take() {
                    screen.dialog = true;
                    screen.question = question;
                    screen.text = Some(text);
                } else {
                    screen.dialog = false;
                }
                screen.revision += 1;
                Ok(ResponseResult::AgentDialogChosen {
                    choice: schema::AgentDialogChooseResult {
                        written: true,
                        reason: None,
                        keys: vec!["enter".into()],
                        observation: schema::AgentDialogObservation {
                            pending_question: false,
                            terminal_id: "terminal".into(),
                            pane_id: "pane".into(),
                            session_id: None,
                            content_revision: 4 + screen.revision,
                            dialog: screen.current_dialog(),
                        },
                    },
                })
            }
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

/// A launching Claude agent in a work room, optionally orchestrated from MASTER.
fn worker(
    orchestrated: bool,
) -> (
    Worker,
    AgentId,
    RoomId,
    Option<AgentId>,
    Arc<Mutex<Screen>>,
    PathBuf,
) {
    // Timestamp resolution alone can collide between parallel fixture threads.
    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "bus-dialogs-{}-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let screen = Arc::new(Mutex::new(Screen::default()));
    let mut worker = Worker::open(dir.clone(), Box::new(FakeServer(screen.clone()))).unwrap();
    worker.dev_enabled = true;
    let mut state = worker.state.clone();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(room, "builder", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: None,
            },
        )
        .unwrap();
    let orchestrator = orchestrated.then(|| {
        let master = state.master_room().unwrap().id;
        let orchestrator = state
            .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
            .unwrap();
        state.bind_orchestrator(orchestrator, room).unwrap();
        orchestrator
    });
    worker.save(state).unwrap();
    (worker, agent, room, orchestrator, screen, dir)
}

/// Messages `author` sent in `room`: (recipients, text), oldest first. A
/// message to the developer has no recipients.
fn sent_by(worker: &Worker, room: RoomId, author: AgentId) -> Vec<(Vec<AgentId>, String)> {
    let mut sent: BTreeMap<PromptId, (Vec<AgentId>, String)> = worker
        .state
        .requests()
        .filter(|request| request.room_id == room)
        .map(|request| &request.prompt)
        .chain(worker.state.room(room).unwrap().notices.iter())
        .filter(|prompt| prompt.author == Author::Agent(author))
        .map(|prompt| {
            (
                prompt.id,
                (
                    prompt.recipient_ids.iter().copied().collect(),
                    prompt.text.clone(),
                ),
            )
        })
        .collect();
    std::mem::take(&mut sent).into_values().collect()
}

fn expected_blocked(worker: &Worker, id: AgentId) -> String {
    format!(
        "{} (agent {}). Blocked, needs help to continue.\nInspect: bus agent dialog {}",
        worker.state.agent(id).unwrap().name,
        id.0,
        id.0
    )
}

/// Worker blocks stay in their work room, and Bus authors no messages.
fn assert_bus_sent_nothing(worker: &Worker) {
    assert!(worker
        .state
        .requests()
        .all(|request| request.prompt.author != Author::Bus));
    let master = worker.state.master_room().unwrap().id;
    let master_room = worker.state.room(master).unwrap();
    assert!(master_room.notices.is_empty(), "{:?}", master_room.notices);
    assert!(master_room.latest_prompt.is_none());
    assert!(worker
        .state
        .requests()
        .all(|request| request.room_id != master));
    for room in worker.state.rooms() {
        assert!(room
            .notices
            .iter()
            .all(|notice| notice.author != Author::Bus));
    }
}

fn polls(worker: &mut Worker, count: usize) {
    for _ in 0..count {
        worker.poll().unwrap();
    }
}

#[test]
fn a_blocked_screen_without_an_open_dialog_does_not_queue_an_orchestrator_request() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(Vec::new(), expected_blocked(&worker, agent))]
    );
    assert!(worker
        .state
        .queued_requests(orchestrator.unwrap())
        .is_empty());
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_blocked_worker_tells_its_orchestrator_once_per_episode() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    screen.lock().unwrap().dialog = true;
    worker.poll().unwrap();
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "a single poll may be a redraw"
    );
    polls(&mut worker, 3);
    let blocked = (vec![orchestrator], expected_blocked(&worker, agent));
    assert_eq!(
        sent_by(&worker, room, agent),
        std::slice::from_ref(&blocked)
    );
    assert_eq!(BLOCKED_MESSAGE, "Blocked, needs help to continue.");
    // It reaches the orchestrator like any message, queued for delivery.
    assert!(worker
        .state
        .requests()
        .any(|request| request.agent_id == orchestrator && request.room_id == room));
    assert!(worker.state.agent(agent).unwrap().dialog);

    // A different question starts another episode without first going idle.
    screen.lock().unwrap().question = true;
    polls(&mut worker, 3);
    screen.lock().unwrap().dialog = false;
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent), [blocked.clone(), blocked]);
    assert_bus_sent_nothing(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn answering_adds_no_echo_and_a_new_dialog_is_a_new_episode() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let fingerprint = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let chosen = worker.choose_dialog_option(agent, 1, &fingerprint).unwrap();
    assert_eq!(chosen["outcome"], "closed");
    let blocked = (vec![orchestrator], expected_blocked(&worker, agent));
    assert_eq!(
        sent_by(&worker, room, agent),
        std::slice::from_ref(&blocked),
        "no answered echo"
    );

    // The identical next prompt appears before any idle status poll.
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent), [blocked.clone(), blocked]);
    assert_bus_sent_nothing(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_same_blocked_screen_is_not_reported_again_after_a_flicker() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    screen.lock().unwrap().blocked = false;
    polls(&mut worker, 3);
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn without_an_orchestrator_the_blocked_worker_tells_the_developer() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(Vec::new(), expected_blocked(&worker, agent))]
    );
    // An agent's message to the developer is news: it counts as unread.
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    assert_eq!(worker.state.requests().count(), 0);
    assert_bus_sent_nothing(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_blocked_orchestrator_tells_the_developer_once_in_master() {
    for wait in ["dialog-id", "question-id", "trust-id", BLOCKED] {
        let (mut worker, _, room, orchestrator, _, dir) = worker(true);
        let orchestrator = orchestrator.unwrap();
        let master = worker.state.master_room().unwrap().id;
        worker
            .notify_dialogs(vec![(orchestrator, Some(wait.into()))])
            .unwrap();
        assert!(sent_by(&worker, master, orchestrator).is_empty());
        notify_steady(&mut worker, orchestrator, Some(wait));
        assert_eq!(
            sent_by(&worker, master, orchestrator),
            [(Vec::new(), expected_blocked(&worker, orchestrator))]
        );
        let master_room = worker.state.room(master).unwrap();
        let notice = &master_room.notices[0];
        assert_eq!(notice.author, Author::Agent(orchestrator));
        assert!(notice.files.is_empty());
        assert_eq!(master_room.unread_count, 1);
        // The agent-authored latest prompt uses the existing MASTER ring path.
        assert_eq!(master_room.latest_prompt.as_ref().unwrap().id, notice.id);
        assert!(master_room.sound_enabled());
        assert!(worker.state.room(room).unwrap().notices.is_empty());
        assert_eq!(worker.state.requests().count(), 0);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn notify_steady(worker: &mut Worker, agent: AgentId, wait: Option<&str>) {
    for _ in 0..2 {
        worker
            .notify_dialogs(vec![(agent, wait.map(str::to_owned))])
            .unwrap();
    }
}

#[test]
fn repeating_an_orchestrators_dialog_in_the_same_episode_adds_no_message() {
    let (mut worker, _, _, orchestrator, _, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    let master = worker.state.master_room().unwrap().id;
    for wait in ["dialog-id", "dialog-id", BLOCKED, "dialog-id"] {
        notify_steady(&mut worker, orchestrator, Some(wait));
    }
    assert_eq!(
        sent_by(&worker, master, orchestrator),
        [(Vec::new(), expected_blocked(&worker, orchestrator))]
    );
    assert_eq!(worker.state.room(master).unwrap().unread_count, 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_new_dialog_while_still_blocked_notifies_workers_and_orchestrators() {
    for (question, text, next_question, next_text) in [
        (false, "Allow command one?", false, "Allow command two?"),
        (true, "What token should Bus use?", false, "Allow command?"),
        (false, "Which layout?", false, "Which color?"),
        (true, "Form one: name and token?", true, "Form two: host?"),
    ] {
        for in_master in [false, true] {
            let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
            let orchestrator = orchestrator.unwrap();
            let (sender, room, recipients) = if in_master {
                let identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
                let mut state = worker.state.clone();
                state.delete_agent(agent).unwrap();
                state
                    .set_agent_runtime_identity(orchestrator, identity)
                    .unwrap();
                let master = state.master_room().unwrap().id;
                worker.save(state).unwrap();
                (orchestrator, master, Vec::new())
            } else {
                (agent, room, vec![orchestrator])
            };
            {
                let mut screen = screen.lock().unwrap();
                screen.dialog = true;
                screen.question = question;
                screen.text = Some(text.into());
            }
            polls(&mut worker, 3);
            assert_eq!(
                worker.state.agent(sender).unwrap().status,
                RuntimeStatus::Blocked
            );
            assert_eq!(sent_by(&worker, room, sender).len(), 1);

            // The next permission/question form appears immediately after the answer.
            screen.lock().unwrap().next_dialog = Some((next_question, next_text.into()));
            let fingerprint = worker.observe_dialog(sender).unwrap()["fingerprint"]
                .as_str()
                .unwrap()
                .to_owned();
            let answered = if question {
                worker
                    .answer_dialog(sender, Some("answer"), false, &fingerprint)
                    .unwrap()
            } else {
                worker
                    .choose_dialog_option(sender, 1, &fingerprint)
                    .unwrap()
            };
            assert_eq!(answered["outcome"], "replaced");
            polls(&mut worker, 3);
            assert_eq!(
                worker.state.agent(sender).unwrap().status,
                RuntimeStatus::Blocked
            );
            assert_eq!(sent_by(&worker, room, sender).len(), 2);

            let blocked = (recipients, expected_blocked(&worker, sender));
            assert_eq!(sent_by(&worker, room, sender), [blocked.clone(), blocked]);
            polls(&mut worker, 3);
            assert_eq!(sent_by(&worker, room, sender).len(), 2);

            // A confirmed close is another boundary, even if the next form
            // has identical text and is shown before the next status poll.
            let fingerprint = worker.observe_dialog(sender).unwrap()["fingerprint"]
                .as_str()
                .unwrap()
                .to_owned();
            let answered = if next_question {
                worker
                    .answer_dialog(sender, Some("answer"), false, &fingerprint)
                    .unwrap()
            } else {
                worker
                    .choose_dialog_option(sender, 1, &fingerprint)
                    .unwrap()
            };
            assert_eq!(answered["outcome"], "closed");
            assert_eq!(sent_by(&worker, room, sender).len(), 2);
            screen.lock().unwrap().dialog = true;
            polls(&mut worker, 3);
            assert_eq!(sent_by(&worker, room, sender).len(), 3);
            drop(worker);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[test]
fn an_unblocked_orchestrator_reports_a_new_dialog_as_a_new_episode() {
    let (mut worker, _, _, orchestrator, _, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    let master = worker.state.master_room().unwrap().id;
    notify_steady(&mut worker, orchestrator, Some("dialog-id"));
    notify_steady(&mut worker, orchestrator, None);
    assert_eq!(sent_by(&worker, master, orchestrator).len(), 1);
    notify_steady(&mut worker, orchestrator, Some("question-id"));
    let blocked = (Vec::new(), expected_blocked(&worker, orchestrator));
    assert_eq!(
        sent_by(&worker, master, orchestrator),
        [blocked.clone(), blocked]
    );
    assert_eq!(worker.state.room(master).unwrap().unread_count, 2);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_flicker_does_not_repeat_an_orchestrators_blocked_message() {
    for wait in ["dialog-id", BLOCKED] {
        let (mut worker, _, _, orchestrator, _, dir) = worker(true);
        let orchestrator = orchestrator.unwrap();
        let master = worker.state.master_room().unwrap().id;
        notify_steady(&mut worker, orchestrator, Some(wait));
        worker.notify_dialogs(vec![(orchestrator, None)]).unwrap();
        notify_steady(&mut worker, orchestrator, Some(wait));
        if wait == BLOCKED {
            // Unreadable blocked screens retain the marker through idle flicker.
            notify_steady(&mut worker, orchestrator, None);
            notify_steady(&mut worker, orchestrator, Some(wait));
        }
        assert_eq!(
            sent_by(&worker, master, orchestrator),
            [(Vec::new(), expected_blocked(&worker, orchestrator))]
        );
        assert_eq!(worker.state.room(master).unwrap().unread_count, 1);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn message_status_names_recipients_waiting_on_a_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    let mut state = worker.state.clone();
    state.set_draft_text(room, "build it").unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state
        .submit_draft(room, crate::messaging::storage::io::now_ms())
        .unwrap()[0];
    worker.save(state).unwrap();
    let message = worker.state.request(request).unwrap().prompt.id;
    let status = |worker: &mut Worker| worker.control_message(message).unwrap();
    assert_eq!(status(&mut worker)["waiting_on_dialog"], json!([]));
    screen.lock().unwrap().dialog = true;
    worker.poll().unwrap();
    let waiting = status(&mut worker);
    assert_eq!(waiting["waiting_on_dialog"], json!([agent.0]));
    assert_eq!(waiting["requests"][0]["dialog"], true);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn blocked_notice_identifies_worker_and_exact_inspection_command() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let messages = sent_by(&worker, room, agent);
    assert_eq!(messages[0].0, vec![orchestrator.unwrap()]);
    assert!(messages[0].1.contains("builder"), "{}", messages[0].1);
    assert!(messages[0]
        .1
        .contains(&format!("bus agent dialog {}", agent.0)));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn message_status_rechecks_a_dialog_that_closed_between_polls() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    let receipt = worker
        .control_send(
            &json!({"room": room.0.to_string(), "to": [agent.0.to_string()], "text": "build it"}),
        )
        .unwrap();
    let message = PromptId(receipt["message_id"].as_u64().unwrap());
    screen.lock().unwrap().dialog = true;
    worker.poll().unwrap();
    screen.lock().unwrap().dialog = false;
    // No status poll: the native observation is already clear.
    let status = worker.control_message(message).unwrap();
    assert_eq!(status["requests"][0]["dialog"], false);
    assert_eq!(status["waiting_on_dialog"], json!([]));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A worker whose queued Codex question starts collapsed. The native list
/// reports its pending-question id until `agent dialog` opens the panel, then
/// the id of the expanded question parsed from the real capture.
struct CollapsedCodexQuestion(Arc<Mutex<bool>>);

impl Transport for CollapsedCodexQuestion {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let expanded = crate::agents::dialog::parse(include_str!(
            "../../../../tests/fixtures/codex-question/expanded.txt"
        ))
        .expect("expanded Codex question parses");
        let mut open = self.0.lock().unwrap();
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList {
                agents: vec![serde_json::from_value(json!({
                    "terminal_id": "terminal", "pane_id": "pane", "name": "bus-r1-a2",
                    "agent": "claude", "workspace_id": "w", "tab_id": "t", "focused": false,
                    "interactive_ready": true, "revision": 1, "agent_status": "blocked",
                    "dialog_id": if *open { expanded.id() } else { "codex:pending_question".into() },
                }))
                .unwrap()],
            }),
            Method::AgentDialogObserve(params) => {
                *open |= params.open_pending_question;
                let mut observed = dialog();
                observed.id = expanded.id();
                Ok(ResponseResult::AgentDialog {
                    observation: schema::AgentDialogObservation {
                        pending_question: !*open,
                        terminal_id: "terminal".into(),
                        pane_id: "pane".into(),
                        session_id: None,
                        content_revision: 4,
                        dialog: open.then_some(observed),
                    },
                })
            }
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

#[test]
fn opening_a_collapsed_codex_question_does_not_repeat_the_blocked_notice() {
    let (mut worker, agent, room, orchestrator, _, dir) = worker(true);
    worker.transport = Box::new(CollapsedCodexQuestion(Arc::default()));
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1, "collapsed question");
    // The orchestrator follows the notice's own inspection command.
    worker.observe_dialog(agent).unwrap();
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(
            vec![orchestrator.unwrap()],
            expected_blocked(&worker, agent)
        )],
        "the same question, now expanded, is not a new blocked episode"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Lists agents like `FakeServer` but cannot read the screen, as while the
/// native server restarts or the pane closes between polls.
struct UnreadableScreen(FakeServer);

impl Transport for UnreadableScreen {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentDialogObserve(_) => Err(TransportError {
                code: None,
                message: "native server unavailable".into(),
                definitely_rejected: false,
            }),
            other => self.0.request(other),
        }
    }
}

#[test]
fn message_status_still_answers_when_a_flagged_recipients_screen_is_unreadable() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    let receipt = worker
        .control_send(
            &json!({"room": room.0.to_string(), "to": [agent.0.to_string()], "text": "build it"}),
        )
        .unwrap();
    let message = PromptId(receipt["message_id"].as_u64().unwrap());
    screen.lock().unwrap().dialog = true;
    worker.poll().unwrap();
    worker.transport = Box::new(UnreadableScreen(FakeServer(screen.clone())));
    let status = worker.control_message(message);
    assert!(
        status.is_ok(),
        "a status read must not fail on a screen refresh: {status:?}"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_the_rooms_own_orchestrator_takes_messages_from_outside_its_room() {
    let (mut worker, agent, room, orchestrator, _, dir) = worker(true);
    let master = worker.state.master_room().unwrap().id;
    let bystander = worker
        .state
        .create_agent(master, "other", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    let draft = |to: AgentId| Draft {
        text: "hi".into(),
        files: Vec::new(),
        recipient_ids: [to].into(),
    };
    assert!(worker
        .state
        .submit_message_from(room, draft(orchestrator.unwrap()), Author::Agent(agent), 1)
        .is_ok());
    assert!(matches!(
        worker
            .state
            .submit_message_from(room, draft(bystander), Author::Agent(agent), 1),
        Err(ModelError::AgentOutsideRoom(id)) if id == bystander
    ));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn collapsed_question_inspection_retries_redraw_without_repeating_notice() {
    struct RedrawDuringOpen {
        inner: CollapsedCodexQuestion,
        stage: u8,
        opens: Arc<Mutex<Vec<bool>>>,
    }
    impl Transport for RedrawDuringOpen {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if let Method::AgentDialogObserve(params) = &method {
                if params.open_pending_question || self.stage > 0 {
                    self.opens
                        .lock()
                        .unwrap()
                        .push(params.open_pending_question);
                    match self.stage {
                        0 => {
                            self.stage = 1;
                            let mut pending = params.clone();
                            pending.open_pending_question = false;
                            return self.inner.request(Method::AgentDialogObserve(pending));
                        }
                        1 => {
                            assert!(!params.open_pending_question);
                            self.stage = 2;
                            *self.inner.0.lock().unwrap() = true;
                            return Err(TransportError {
                                code: Some("dialog_observation_unavailable".into()),
                                message: "Agent screen changed while it was being observed".into(),
                                definitely_rejected: true,
                            });
                        }
                        _ => {}
                    }
                }
            }
            self.inner.request(method)
        }
    }
    let (mut worker, agent, room, _, _, dir) = worker(true);
    let opens = Arc::default();
    worker.transport = Box::new(RedrawDuringOpen {
        inner: CollapsedCodexQuestion(Arc::default()),
        stage: 0,
        opens: Arc::clone(&opens),
    });
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    let observed = worker.observe_dialog(agent);
    polls(&mut worker, 3);
    let notices = sent_by(&worker, room, agent).len();
    assert!(
        observed.is_ok(),
        "redraw aborted inspection: {observed:?}; notices={notices}"
    );
    assert!(observed.unwrap()["fingerprint"].is_string());
    assert_eq!(
        notices, 1,
        "opening the same question must not notify twice"
    );
    let opens = opens.lock().unwrap();
    assert_eq!(opens.iter().filter(|open| **open).count(), 1);
    assert!(opens.len() >= 3, "must read again after the redraw error");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

fn redraw_error() -> TransportError {
    TransportError {
        code: Some("dialog_observation_unavailable".into()),
        message: "Agent screen changed while it was being observed".into(),
        definitely_rejected: true,
    }
}

#[test]
fn collapsed_question_inspection_retries_a_redraw_on_the_opening_read() {
    struct RedrawOnOpen {
        inner: CollapsedCodexQuestion,
        observes: Arc<Mutex<Vec<bool>>>,
    }
    impl Transport for RedrawOnOpen {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if let Method::AgentDialogObserve(params) = &method {
                self.observes
                    .lock()
                    .unwrap()
                    .push(params.open_pending_question);
                if params.open_pending_question {
                    // The server queued Shift+Left, then Codex's redraw broke the snapshot.
                    *self.inner.0.lock().unwrap() = true;
                    return Err(redraw_error());
                }
            }
            self.inner.request(method)
        }
    }
    let (mut worker, agent, room, _, _, dir) = worker(true);
    let observes = Arc::default();
    worker.transport = Box::new(RedrawOnOpen {
        inner: CollapsedCodexQuestion(Arc::default()),
        observes: Arc::clone(&observes),
    });
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    let observed = worker.observe_dialog(agent);
    polls(&mut worker, 3);
    assert!(observed.is_ok(), "{observed:?}");
    assert!(observed.unwrap()["fingerprint"].is_string());
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    assert_eq!(*observes.lock().unwrap(), [true, false]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dialog_inspection_gives_up_after_the_bounded_wait_when_redraws_never_settle() {
    struct AlwaysRedrawing {
        inner: CollapsedCodexQuestion,
        observes: Arc<Mutex<Vec<bool>>>,
    }
    impl Transport for AlwaysRedrawing {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if let Method::AgentDialogObserve(params) = &method {
                self.observes
                    .lock()
                    .unwrap()
                    .push(params.open_pending_question);
                return Err(redraw_error());
            }
            self.inner.request(method)
        }
    }
    let (mut worker, agent, room, _, _, dir) = worker(true);
    let observes = Arc::default();
    worker.transport = Box::new(AlwaysRedrawing {
        inner: CollapsedCodexQuestion(Arc::default()),
        observes: Arc::clone(&observes),
    });
    polls(&mut worker, 3);
    let notice = worker.state.agent(agent).unwrap().dialog_notice.clone();
    let started = std::time::Instant::now();
    let observed = worker.observe_dialog(agent);
    assert!(observed.is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let observes = observes.lock().unwrap();
    assert_eq!(observes.len(), 21, "one opening read plus 20 settle reads");
    assert_eq!(observes.iter().filter(|open| **open).count(), 1);
    assert_eq!(worker.state.agent(agent).unwrap().dialog_notice, notice);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn collapsed_question_inspection_waits_past_a_torn_frame_without_repeating_notice() {
    // Codex replaces the collapsed queue with the opened question; a snapshot
    // between the two shows neither, while the screen is still being drawn.
    struct TornFrameDuringOpen {
        inner: CollapsedCodexQuestion,
        stage: u8,
    }
    impl Transport for TornFrameDuringOpen {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if let Method::AgentDialogObserve(params) = &method {
                match self.stage {
                    0 if params.open_pending_question => {
                        self.stage = 1;
                        let mut collapsed = params.clone();
                        collapsed.open_pending_question = false;
                        return self.inner.request(Method::AgentDialogObserve(collapsed));
                    }
                    1 => {
                        self.stage = 2;
                        *self.inner.0.lock().unwrap() = true;
                        return Ok(ResponseResult::AgentDialog {
                            observation: schema::AgentDialogObservation {
                                pending_question: false,
                                terminal_id: "terminal".into(),
                                pane_id: "pane".into(),
                                session_id: None,
                                content_revision: 5,
                                dialog: None,
                            },
                        });
                    }
                    _ => {}
                }
            }
            self.inner.request(method)
        }
    }
    let (mut worker, agent, room, _, _, dir) = worker(true);
    worker.transport = Box::new(TornFrameDuringOpen {
        inner: CollapsedCodexQuestion(Arc::default()),
        stage: 0,
    });
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    let observed = worker.observe_dialog(agent).unwrap();
    polls(&mut worker, 3);
    let notices = sent_by(&worker, room, agent).len();
    assert_eq!(
        notices, 1,
        "a torn frame ended inspection with dialog={}; the opened question notified again",
        observed["dialog"]
    );
    assert!(observed["fingerprint"].is_string());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn collapsed_question_inspection_bounds_the_wait_for_an_empty_replacement_frame() {
    struct EmptyAfterOpen {
        inner: CollapsedCodexQuestion,
        observes: Arc<Mutex<Vec<bool>>>,
    }
    impl Transport for EmptyAfterOpen {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if let Method::AgentDialogObserve(params) = &method {
                self.observes
                    .lock()
                    .unwrap()
                    .push(params.open_pending_question);
                let response = self.inner.request(method)?;
                let ResponseResult::AgentDialog { mut observation } = response else {
                    panic!("dialog response");
                };
                observation.pending_question = false;
                observation.dialog = None;
                return Ok(ResponseResult::AgentDialog { observation });
            }
            self.inner.request(method)
        }
    }
    let (mut worker, agent, room, _, _, dir) = worker(true);
    let observes = Arc::default();
    worker.transport = Box::new(EmptyAfterOpen {
        inner: CollapsedCodexQuestion(Arc::default()),
        observes: Arc::clone(&observes),
    });
    polls(&mut worker, 3);
    let notice = worker.state.agent(agent).unwrap().dialog_notice.clone();
    assert_eq!(
        notice.as_deref(),
        Some(crate::agents::dialog::CODEX_PENDING_QUESTION_ID)
    );
    let observed = worker.observe_dialog(agent).unwrap();
    assert!(observed["dialog"].is_null());
    assert!(observed["fingerprint"].is_null());
    let observes = observes.lock().unwrap();
    assert_eq!(
        observes.len(),
        21,
        "pending episode waits through the full bound even if the opening read is empty"
    );
    assert_eq!(observes.iter().filter(|open| **open).count(), 1);
    assert_eq!(worker.state.agent(agent).unwrap().dialog_notice, notice);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
