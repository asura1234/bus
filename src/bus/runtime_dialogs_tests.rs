use super::*;
use crate::bus::transport::TransportError;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// What the fake server shows for the watched agent's screen.
#[derive(Clone, Default)]
struct Screen {
    dialog: bool,
    blocked: bool,
}

struct FakeServer(Arc<Mutex<Screen>>);

fn dialog() -> schema::AgentDialog {
    schema::AgentDialog {
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
                    dialog: screen.dialog.then(dialog),
                },
            }),
            Method::AgentDialogChoose(_) => {
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
        state
            .set_agent_orchestrates(orchestrator, Some(room))
            .unwrap();
        orchestrator
    });
    worker.save(state).unwrap();
    (worker, agent, room, orchestrator, screen, dir)
}

/// Bus messages delivered to `agent`, oldest first.
fn messages_to(worker: &Worker, agent: AgentId) -> Vec<String> {
    let mut prompts: Vec<_> = worker
        .state
        .requests()
        .filter(|request| request.agent_id == agent && request.prompt.author == Author::Bus)
        .map(|request| (request.prompt.id, request.prompt.text.clone()))
        .collect();
    prompts.sort();
    prompts.into_iter().map(|(_, text)| text).collect()
}

fn notices(worker: &Worker, room: RoomId) -> Vec<String> {
    let room = worker.state.room(room).unwrap();
    room.notices
        .iter()
        .map(|notice| notice.text.clone())
        .collect()
}

fn polls(worker: &mut Worker, count: usize) {
    for _ in 0..count {
        worker.poll().unwrap();
    }
}

#[test]
fn orchestrator_is_told_once_per_dialog_and_when_it_closes_on_its_own() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    screen.lock().unwrap().dialog = true;
    worker.poll().unwrap();
    assert!(
        messages_to(&worker, orchestrator).is_empty(),
        "a single poll may be a redraw"
    );
    polls(&mut worker, 3);
    let sent = messages_to(&worker, orchestrator);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let notice = &sent[0];
    assert!(
        notice.starts_with(&format!(
            "builder (agent {}) in room work is waiting on a dialog",
            agent.0
        )),
        "{notice}"
    );
    assert!(
        notice.contains("Do you want to proceed?\n\n1. Yes (selected)\n2. No\nEsc to cancel"),
        "{notice}"
    );
    assert!(
        notice.contains(&format!(
            "bus agent choose {} --option N --fingerprint d1.",
            agent.0
        )),
        "{notice}"
    );
    let master = worker.state.master_room().unwrap().id;
    assert!(worker
        .state
        .requests()
        .all(|request| request.room_id != master || request.agent_id == orchestrator));
    assert!(worker.state.agent(agent).unwrap().dialog);
    assert!(notices(&worker, room).is_empty());

    screen.lock().unwrap().dialog = false;
    polls(&mut worker, 3);
    let sent = messages_to(&worker, orchestrator);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(
        sent[1].contains("closed without an answer through Bus"),
        "{}",
        sent[1]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn without_an_orchestrator_the_human_gets_the_notice_and_answers_need_no_follow_up() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(posted[0].contains("is waiting on a dialog"));
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    assert_eq!(
        worker.state.requests().count(),
        0,
        "a notice is delivered to no agent"
    );

    let fingerprint = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let chosen = worker.choose_dialog_option(agent, 1, &fingerprint).unwrap();
    assert_eq!(chosen["outcome"], "closed");
    polls(&mut worker, 3);
    assert_eq!(
        notices(&worker, room).len(),
        1,
        "an answered dialog closes quietly"
    );

    // A blocked screen without a readable dialog still gets reported.
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 2);
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 2, "{posted:?}");
    assert!(posted[1].contains(&format!("bus agent read {} --source visible", agent.0)));
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
