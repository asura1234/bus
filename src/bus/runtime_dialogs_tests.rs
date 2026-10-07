use super::*;
use crate::bus::transport::TransportError;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// What the fake server shows for the watched agent's screen.
#[derive(Clone, Default)]
struct Screen {
    dialog: bool,
    blocked: bool,
    question: bool,
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
                    "dialog_id": screen.dialog.then_some("dialog-id"),
                }))
                .unwrap()],
            }),
            Method::AgentDialogObserve(_) => Ok(ResponseResult::AgentDialog {
                observation: schema::AgentDialogObservation {
                    terminal_id: "terminal".into(),
                    pane_id: "pane".into(),
                    session_id: None,
                    content_revision: 4,
                    dialog: screen.dialog.then(|| {
                        let mut dialog=dialog();
                        if screen.question {
                            dialog.kind=schema::AgentDialogKind::Question;
                            dialog.text="What token should Bus use?".into();
                            dialog.options.clear();
                        }
                        dialog
                    }),
                },
            }),
            Method::AgentDialogChoose(_) | Method::AgentDialogAnswer(_) => {
                self.0.lock().unwrap().dialog = false;
                Ok(ResponseResult::AgentDialogChosen {
                    choice: schema::AgentDialogChooseResult {
                        written: true,
                        reason: None,
                        keys: vec!["enter".into()],
                        observation: schema::AgentDialogObservation {
                            terminal_id: "terminal".into(),
                            pane_id: "pane".into(),
                            session_id: None,
                            content_revision: 5,
                            dialog: None,
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
    let dir = std::env::temp_dir().join(format!(
        "bus-dialogs-{}-{}",
        std::process::id(),
        crate::bus::io::now_ns()
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
/// message to the Human has no recipients.
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

/// Bus is not an agent: it authors nothing, and MASTER never hears of a block.
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
    let blocked = (vec![orchestrator], BLOCKED_MESSAGE.to_owned());
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

    // Still blocked, now on a question and then a blocked screen: same episode.
    screen.lock().unwrap().question = true;
    polls(&mut worker, 3);
    screen.lock().unwrap().dialog = false;
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        std::slice::from_ref(&blocked)
    );
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
    polls(&mut worker, 3);
    let blocked = (vec![orchestrator], BLOCKED_MESSAGE.to_owned());
    assert_eq!(
        sent_by(&worker, room, agent),
        std::slice::from_ref(&blocked),
        "no answered echo"
    );

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
fn without_an_orchestrator_the_blocked_worker_tells_the_human() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(Vec::new(), BLOCKED_MESSAGE.to_owned())]
    );
    // An agent's message to the Human is news: it counts as unread.
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    assert_eq!(worker.state.requests().count(), 0);
    assert_bus_sent_nothing(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_blocked_agent_in_master_posts_nothing() {
    let (mut worker, _, _, orchestrator, _, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    worker
        .notify_dialogs(vec![(orchestrator, Some("dialog-id".into()))])
        .unwrap();
    worker
        .notify_dialogs(vec![(orchestrator, Some("dialog-id".into()))])
        .unwrap();
    assert_eq!(
        worker
            .state
            .agent(orchestrator)
            .unwrap()
            .dialog_notice
            .as_deref(),
        Some("dialog-id")
    );
    assert_eq!(worker.state.requests().count(), 0);
    assert_bus_sent_nothing(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn message_status_names_recipients_waiting_on_a_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    let mut state = worker.state.clone();
    state.set_draft_text(room, "build it").unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, 2).unwrap()[0];
    worker.save(state).unwrap();
    let message = worker.state.request(request).unwrap().prompt.id;
    let status = |worker: &mut Worker| worker.dev_message(message).unwrap();
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
