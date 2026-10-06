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

#[test]
fn notice_names_the_command_and_leaves_the_fingerprint_out() {
    let observed = json!({
        "dialog": {
            "text": "Would you like to run the following command?\n\n$ printf muse-safe-probe",
            "options": [
                {"number": 1, "label": "Allow once", "selected": true},
                {"number": 2, "label": "Abort", "selected": false}
            ],
            "hint": "Press enter to confirm or esc to cancel"
        },
        "fingerprint": "d1.eyJabc"
    });
    let notice = dialog_notice(AgentId(61), "Codex", "dev", &observed);
    assert_eq!(
        notice,
        "Codex in dev (agent 61)\nCodex wants to run: printf muse-safe-probe\n1. Allow once (selected)\n2. Abort\n\nAnswer: bus agent dialog 61, then bus agent choose 61 --option N"
    );
}

#[test]
fn approval_notice_retains_every_command_line_and_the_reason() {
    let observed = json!({"dialog":{
        "text":"Would you like to run the following command?\n\nReason: Capture Claude's custom answer control\nin the isolated Bus.\n\n$ sed -n 1280,1344p src/app/api/agents.rs\nsed -n 385,402p src/bus/control_cli.rs\npython3 /private/tmp/bus-question-probe.py /private/tmp/bq cli ...",
        "options":[], "hint":"Press enter to confirm or esc to cancel"
    }, "fingerprint":"SECRET"});
    let notice = dialog_notice(AgentId(61), "Codex", "dev", &observed);
    assert!(notice.contains("Codex wants to run: sed -n 1280,1344p src/app/api/agents.rs\nsed -n 385,402p src/bus/control_cli.rs\npython3 /private/tmp/bus-question-probe.py /private/tmp/bq cli ..."), "{notice}");
    assert!(
        notice.contains("Reason: Capture Claude's custom answer control\nin the isolated Bus."),
        "{notice}"
    );
    assert!(!notice.contains("SECRET"));
    let commands = (0..16)
        .map(|n| format!("printf command-{n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let screen=format!("Would you like to run the following command?\n\nReason: Check every line in this native prompt\n\n$ {commands}\n\n› 1. Allow once\n  2. Abort\n\nPress enter to confirm or esc to cancel\n");
    let parsed = crate::detect::dialog::parse(&screen).unwrap();
    let observed = json!({"dialog":{"text":parsed.text,"options":[]}});
    let notice = dialog_notice(AgentId(61), "Codex", "dev", &observed);
    assert!(notice.contains(&commands), "{notice}");
    assert!(
        notice.contains("Reason: Check every line in this native prompt"),
        "{notice}"
    );
}

#[test]
fn free_text_notice_retains_the_question_and_names_answer_without_a_fingerprint() {
    let observed = json!({"dialog":{"kind":"question","text":"Which token?\nPlease give the complete value.","options":[],"hint":"enter submit ctrl+] skip shift+→ main prompt"},"fingerprint":"SECRET"});
    let notice = dialog_notice(AgentId(61), "Codex", "dev", &observed);
    assert_eq!(notice, "Codex in dev (agent 61)\nWhich token?\nPlease give the complete value.\n\nAnswer: bus agent dialog 61, then bus agent answer 61 --text \"...\" or bus agent answer 61 --skip");
    assert!(!notice.contains("SECRET"));
}

#[test]
fn the_same_blocked_notice_is_not_posted_again_after_a_flicker() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 3);
    assert_eq!(notices(&worker, room).len(), 1);

    screen.lock().unwrap().blocked = false;
    polls(&mut worker, 3);
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 3);
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(posted[0].contains(&format!("bus agent read {} --source visible", agent.0)));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
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
    let expected = format!(
        "builder in work (agent {id})\nDo you want to proceed?\n1. Yes (selected)\n2. No\n\nAnswer: bus agent dialog {id}, then bus agent choose {id} --option N",
        id = agent.0
    );
    assert_eq!(notice, &expected);
    assert!(!notice.contains("fingerprint"), "{notice}");
    assert!(!notice.contains("Esc to cancel"), "{notice}");
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
    assert_eq!(sent[1], "answered");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn master_receives_a_text_question_and_a_generic_answer_notice() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    screen.lock().unwrap().dialog = true;
    screen.lock().unwrap().question = true;
    polls(&mut worker, 3);
    let sent = messages_to(&worker, orchestrator);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("What token should Bus use?"));
    assert!(sent[0].contains(&format!("bus agent answer {} --text", agent.0)));
    assert!(!sent[0].contains("fingerprint"));
    assert!(notices(&worker, room).is_empty());
    let fingerprint = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        worker
            .answer_dialog(agent, Some("token"), false, &fingerprint)
            .unwrap()["outcome"],
        "closed"
    );
    polls(&mut worker, 3);
    assert_eq!(messages_to(&worker, orchestrator)[1], "answered");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn without_an_orchestrator_the_human_gets_the_notice_and_a_one_line_answer() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(posted[0].contains("Do you want to proceed?"));
    assert!(posted[0].contains(&format!(
        "Answer: bus agent dialog {id}, then bus agent choose {id} --option N",
        id = agent.0
    )));
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
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 2, "{posted:?}");
    assert_eq!(posted[1], "answered: option 1");

    // A blocked screen without a readable dialog still gets reported.
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 2);
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 3, "{posted:?}");
    assert!(posted[2].contains(&format!("bus agent read {} --source visible", agent.0)));
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
