use super::*;
use crate::messaging::native::TransportError;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What the fake server shows for the watched agent's screen.
#[derive(Clone, Default)]
struct Screen {
    pending_question: bool,
    settle_errors: u32,
    answer_sent: bool,
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
                free_text: false,
            },
            schema::AgentDialogOption {
                number: 2,
                label: "No".into(),
                selected: false,
                free_text: false,
            },
        ],
        hint: Some("Esc to cancel".into()),
        id: "dialog-id".into(),
        digest: "dialog-digest".into(),
        free_text: None,
        tabs: Vec::new(),
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
        if matches!(&method, Method::AgentDialogObserve(_))
            && screen.answer_sent
            && screen.settle_errors > 0
        {
            self.0.lock().unwrap().settle_errors -= 1;
            return Err(TransportError {
                code: Some("dialog_observation_unavailable".into()),
                message: "Agent screen changed while it was being observed".into(),
                definitely_rejected: true,
            });
        }
        match method {
            Method::AgentGet(params) => Ok(ResponseResult::AgentInfo {
                agent: serde_json::from_value(json!({
                    "terminal_id": "terminal", "pane_id": params.target, "name": "bus-r1-a2",
                    "agent": "claude", "workspace_id": "w", "tab_id": "t", "focused": false,
                    "interactive_ready": true, "revision": 1,
                    "agent_status": if screen.dialog || screen.blocked { "blocked" } else { "idle" },
                    "dialog_id": screen.current_dialog().map(|dialog| dialog.id),
                })).unwrap(),
            }),
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
            Method::AgentDialogObserve(params) => Ok(ResponseResult::AgentDialog {
                observation: schema::AgentDialogObservation {
                    pending_question: screen.pending_question,
                    terminal_id: "terminal".into(),
                    // Every watched agent shows the same screen in its own pane.
                    pane_id: params.target,
                    session_id: None,
                    content_revision: 4 + screen.revision,
                    dialog: screen.current_dialog(),
                },
            }),
            Method::AgentDialogChoose(_) | Method::AgentDialogAnswer(_) => {
                let mut screen = self.0.lock().unwrap();
                screen.answer_sent = true;
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

/// Bus notices about `about` in `room`: (recipients, text), oldest first. A
/// notice to the developer has no recipients.
fn sent_by(worker: &Worker, room: RoomId, about: AgentId) -> Vec<(Vec<AgentId>, String)> {
    let name = &worker.state.agent(about).unwrap().name;
    let prefix = format!("{name}.");
    let mut sent: BTreeMap<PromptId, (Vec<AgentId>, String)> = worker
        .state
        .requests()
        .filter(|request| request.room_id == room)
        .map(|request| &request.prompt)
        .chain(worker.state.room(room).unwrap().notices.iter())
        .filter(|prompt| {
            prompt.author == Author::Bus
                && (prompt.text.starts_with(&prefix)
                    || prompt
                        .attention_origin
                        .as_ref()
                        .is_some_and(|origin| origin.agent_id == about))
        })
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
    let body = format!(
        "{}. Blocked, needs help to continue.\nInspect: bus agent dialog '{}'",
        worker.state.agent(id).unwrap().name,
        worker.state.agent(id).unwrap().name
    );
    let agent = worker.state.agent(id).unwrap();
    if worker
        .state
        .is_developer_attention_author(agent.room_id, &Author::Agent(id))
    {
        format!("{} {body}", crate::messaging::attention::TAG)
    } else {
        body
    }
}

#[test]
fn grok_login_reason_is_inspectable_when_orchestrator_is_notified() {
    for capture in [
        include_str!("../../../../tests/fixtures/grok-login/native-1.0.70-error.txt"),
        include_str!("../../../../tests/fixtures/grok-login/native-1.0.70-connecting.txt"),
        include_str!("../../../../tests/fixtures/grok-login/native-1.0.70-error-71col.txt"),
        include_str!("../../../../tests/fixtures/grok-login/native-1.0.70-error-61col.txt"),
    ] {
        let parsed = crate::agents::dialog::parse(capture).expect("native login dialog");
        let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
        {
            let mut screen = screen.lock().unwrap();
            screen.dialog = true;
            screen.blocked = true;
            screen.text = Some(parsed.text.clone());
        }
        for _ in 0..3 {
            worker.poll().unwrap();
        }
        let notices = sent_by(&worker, room, agent);
        assert_eq!(
            notices,
            vec![(
                vec![orchestrator.unwrap()],
                expected_blocked(&worker, agent)
            )]
        );
        let observation = worker
            .native_dialog("pane", "terminal", None, false)
            .unwrap();
        assert_eq!(observation.dialog.unwrap().text, parsed.text);
        assert!(parsed.text.contains("Grok login required"));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// Worker blocks stay in their work room, signed by Bus, never by the worker.
fn assert_only_bus_signed_notices(worker: &Worker) {
    assert!(worker
        .state
        .requests()
        .all(|request| !matches!(request.prompt.author, Author::Agent(_))));
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
            .all(|notice| !matches!(notice.author, Author::Agent(_))));
    }
}

fn polls(worker: &mut Worker, count: usize) {
    for _ in 0..count {
        worker.poll().unwrap();
    }
}

#[test]
fn parser_null_blocked_worker_notifies_its_orchestrator_once() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    screen.lock().unwrap().blocked = true;
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(
            vec![orchestrator.unwrap()],
            expected_blocked(&worker, agent)
        )]
    );
    assert!(worker
        .native_dialog("pane", "terminal", None, false)
        .unwrap()
        .dialog
        .is_none());
    assert!(
        !screen.lock().unwrap().answer_sent,
        "an unparsed screen must receive no automatic key"
    );
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
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
    assert_only_bus_signed_notices(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn answering_retires_old_help_and_a_new_dialog_is_a_new_episode() {
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
        "closing posts nothing"
    );

    // The identical next prompt appears before any idle status poll.
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent), [blocked.clone(), blocked]);
    assert_only_bus_signed_notices(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_same_blocked_screen_is_not_reported_again_after_a_flicker() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    screen.lock().unwrap().dialog = false;
    polls(&mut worker, 1);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_undelivered_blocked_notice_keeps_its_failure_and_reaches_the_developer_once() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let blocked = expected_blocked(&worker, agent);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(vec![orchestrator], blocked.clone())]
    );
    // The orchestrator never takes its queue; the notice expires untyped.
    let now = crate::messaging::storage::io::now_ms();
    worker
        .expire_queued_requests(now + crate::messaging::model::QUEUED_STALL_MS + 1)
        .unwrap();
    worker
        .expire_unconfirmed_submissions(now + crate::messaging::model::QUEUED_STALL_MS + 1)
        .unwrap();
    let notice = worker
        .state
        .requests()
        .find(|request| request.agent_id == orchestrator)
        .unwrap();
    assert_eq!(
        notice.phase,
        crate::messaging::model::RequestPhase::Abandoned
    );
    assert_eq!(
        notice.failure_reason.as_deref(),
        Some("hook_setup_unconfirmed")
    );
    assert!(notice.completed_at_ms.is_some());
    let fallback =
        format!("{blocked}\nThe orchestrator did not receive this (hook_setup_unconfirmed).");
    assert_eq!(
        sent_by(&worker, room, agent),
        [
            (vec![orchestrator], blocked.clone()),
            (Vec::new(), fallback.clone())
        ]
    );
    // Escalation does not close the dialog or post duplicate help on later polls.
    polls(&mut worker, 3);
    assert!(worker.state.agent(agent).unwrap().dialog);
    assert_eq!(sent_by(&worker, room, agent).len(), 2);
    screen.lock().unwrap().dialog = false;
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(vec![orchestrator], blocked), (Vec::new(), fallback)]
    );
    assert!(worker
        .state
        .room(room)
        .unwrap()
        .notices
        .iter()
        .all(|notice| notice.author == Author::Bus));
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
    // A Bus notice to the developer is news: it counts as unread.
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    assert_eq!(worker.state.requests().count(), 0);
    assert_only_bus_signed_notices(&worker);
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
        assert_eq!(notice.author, Author::Bus);
        assert_eq!(
            notice.attention_origin.as_ref().unwrap().agent_id,
            orchestrator
        );
        assert!(notice.files.is_empty());
        assert_eq!(master_room.unread_count, 1);
        // The Bus-authored latest prompt uses the existing MASTER ring path.
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
                state.delete_agent(agent, 0).unwrap();
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
    assert!(messages[0].1.contains("bus agent dialog 'builder'"));
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
    assert_eq!(
        *observes.lock().unwrap(),
        [true, false, false, false],
        "one opening read, its retry, and two read-only notice rechecks"
    );
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

#[test]
fn message_status_writes_nothing_when_no_dialog_changed() {
    let (mut worker, agent, room, _, screen, dir) = worker(false);
    let receipt = worker
        .control_send(
            &json!({"room": room.0.to_string(), "to": [agent.0.to_string()], "text": "build it"}),
        )
        .unwrap();
    let message = PromptId(receipt["message_id"].as_u64().unwrap());
    screen.lock().unwrap().dialog = true;
    worker.poll().unwrap();
    assert!(worker.state.agent(agent).unwrap().dialog);
    let writes = worker.store.writes();
    for _ in 0..3 {
        let status = worker.control_message(message).unwrap();
        assert_eq!(status["waiting_on_dialog"], json!([agent.0]));
    }
    assert_eq!(
        worker.store.writes(),
        writes,
        "a status poll that re-observes the same dialog must not rewrite state.json"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn message_status_saves_once_when_several_recipients_dialogs_closed() {
    let (mut worker, agent, room, _, _, dir) = worker(false);
    let mut state = worker.state.clone();
    let other = state
        .create_agent(room, "tester", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            other,
            AgentRuntimeIdentity {
                launch_id: Some("launch-2".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane-2".into()),
                session_id: None,
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    let receipt = worker
        .control_send(&json!({
            "room": room.0.to_string(),
            "to": [agent.0.to_string(), other.0.to_string()],
            "text": "build it"
        }))
        .unwrap();
    let message = PromptId(receipt["message_id"].as_u64().unwrap());
    // Both were last seen blocked on a dialog; the screens have since cleared.
    let mut state = worker.state.clone();
    state
        .observe_dialog(agent, true, state.agent(agent).unwrap().observed_at_ms)
        .unwrap();
    state
        .observe_dialog(other, true, state.agent(other).unwrap().observed_at_ms)
        .unwrap();
    worker.save(state).unwrap();
    let writes = worker.store.writes();
    let status = worker.control_message(message).unwrap();
    assert_eq!(status["waiting_on_dialog"], json!([]));
    assert!(!worker.state.agent(agent).unwrap().dialog);
    assert!(!worker.state.agent(other).unwrap().dialog);
    assert_eq!(
        worker.store.writes(),
        writes + 1,
        "one status call persists all of its dialog observations in one write"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn choosing_reports_closed_after_transient_redraw() {
    let (mut worker, agent, _, _, screen, dir) = worker(false);
    screen.lock().unwrap().dialog = true;
    let fingerprint = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    screen.lock().unwrap().settle_errors = 2;
    let result = worker.choose_dialog_option(agent, 1, &fingerprint).unwrap();
    assert_eq!(result["outcome"], "closed");
    assert!(result["dialog"].is_null());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn settled_dialog_does_not_send_a_stale_blocked_notice() {
    let (mut worker, agent, room, _, _, dir) = worker(true);
    // Two cached AgentList observations still describe the old dialog, but
    // a native inspection now sees the approval completed.
    notify_steady(&mut worker, agent, Some("old-dialog"));
    assert!(sent_by(&worker, room, agent).is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn manual_dialog_close_retires_the_notice_silently() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    screen.lock().unwrap().dialog = false;
    polls(&mut worker, 4);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_fresh_native_dialog_close_suppresses_fallback_before_episode_debounce_finishes() {
    let (mut worker, agent, _room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let Some(crate::messaging::model::DialogNoticeTo::Orchestrator(notice)) =
        worker.state.agent(agent).unwrap().dialog_notice_to
    else {
        panic!("the open dialog must have an orchestrator notice");
    };
    let deadline = worker.state.request(notice).unwrap().prompt.submitted_at_ms
        + crate::messaging::model::QUEUED_STALL_MS;
    // Tick closes queued stalls first. Its fresh native poll then sees that
    // the question has gone, before post-reconciliation episode escalation.
    worker.expire_queued_requests(deadline).unwrap();
    screen.lock().unwrap().dialog = false;
    worker.poll().unwrap();
    assert!(!worker.state.agent(agent).unwrap().dialog);
    worker.expire_unconfirmed_submissions(deadline).unwrap();
    let route = worker.state.agent(agent).unwrap().dialog_notice_to;
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_ne!(
        route,
        Some(crate::messaging::model::DialogNoticeTo::Developer),
        "a fresh closed-dialog observation must win over fallback at the bound"
    );
}

#[test]
fn closed_dialog_resolves_help_even_while_blocked_metadata_stays_cached() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    let episode = screen.lock().unwrap().current_dialog().unwrap().id;
    notify_steady(&mut worker, agent, Some(&episode));
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    screen.lock().unwrap().dialog = false;
    notify_steady(&mut worker, agent, Some(&episode));
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    // The stale generic blocked fallback must not resurrect the settled help.
    notify_steady(&mut worker, agent, Some(BLOCKED));
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncertain_close_retains_help_until_a_readable_observation_confirms_settlement() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let episode = worker.state.agent(agent).unwrap().dialog_notice.clone();
    let fingerprint = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    screen.lock().unwrap().settle_errors = 22;
    let result = worker.choose_dialog_option(agent, 1, &fingerprint).unwrap();
    assert_eq!(result["outcome"], "unknown");
    assert_eq!(worker.state.agent(agent).unwrap().dialog_notice, episode);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    notify_steady(&mut worker, agent, episode.as_deref());
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    notify_steady(&mut worker, agent, episode.as_deref());
    assert_eq!(worker.state.agent(agent).unwrap().dialog_notice, None);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn closing_a_dialog_through_choose_posts_no_resolved_notice() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    let fingerprint = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let result = worker.choose_dialog_option(agent, 1, &fingerprint).unwrap();
    assert_eq!(result["outcome"], "closed");
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    // The answerer already knows; only the blocked notice was sent.
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn absent_metadata_keeps_help_while_a_native_dialog_or_question_remains() {
    for pending_question in [false, true] {
        let (mut worker, agent, room, _, screen, dir) = worker(true);
        screen.lock().unwrap().dialog = true;
        polls(&mut worker, 3);
        let episode = worker.state.agent(agent).unwrap().dialog_notice.clone();
        if pending_question {
            let mut current = screen.lock().unwrap();
            current.dialog = false;
            current.pending_question = true;
        }
        notify_steady(&mut worker, agent, None);
        assert_eq!(worker.state.agent(agent).unwrap().dialog_notice, episode);
        assert_eq!(sent_by(&worker, room, agent).len(), 1);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn a_due_fallback_checks_the_native_screen_even_when_list_metadata_still_says_open() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let Some(crate::messaging::model::DialogNoticeTo::Orchestrator(notice)) =
        worker.state.agent(agent).unwrap().dialog_notice_to
    else {
        panic!("missing notice")
    };
    let deadline = worker.state.request(notice).unwrap().prompt.submitted_at_ms
        + crate::messaging::model::QUEUED_STALL_MS;
    worker.expire_queued_requests(deadline).unwrap();
    screen.lock().unwrap().dialog = false;
    // No new AgentList poll: its open flag and the episode are both stale.
    assert!(worker.state.agent(agent).unwrap().dialog);
    for at in [
        deadline,
        deadline + crate::messaging::coordinator::expiry::DIALOG_OBSERVATION_GRACE_MS + 86_400_000,
    ] {
        worker.expire_unconfirmed_submissions(at).unwrap();
        assert_eq!(
            worker.state.agent(agent).unwrap().dialog_notice_to,
            Some(crate::messaging::model::DialogNoticeTo::Orchestrator(
                notice
            ))
        );
        assert_eq!(sent_by(&worker, room, agent).len(), 1);
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_due_fallback_reaches_the_developer_for_an_open_native_dialog_or_pending_question() {
    for pending in [false, true] {
        let (mut worker, agent, room, _, screen, dir) = worker(true);
        screen.lock().unwrap().dialog = true;
        polls(&mut worker, 3);
        let Some(crate::messaging::model::DialogNoticeTo::Orchestrator(notice)) =
            worker.state.agent(agent).unwrap().dialog_notice_to
        else {
            panic!("missing notice")
        };
        let deadline = worker.state.request(notice).unwrap().prompt.submitted_at_ms
            + crate::messaging::model::QUEUED_STALL_MS;
        if pending {
            let mut current = screen.lock().unwrap();
            current.dialog = false;
            current.pending_question = true;
        }
        notify_steady(&mut worker, agent, None);
        let mut state = worker.state.clone();
        state.observe_dialog(agent, false, deadline).unwrap();
        worker.save(state).unwrap();
        worker.expire_queued_requests(deadline).unwrap();
        worker.expire_unconfirmed_submissions(deadline).unwrap();
        assert_eq!(
            worker.state.agent(agent).unwrap().dialog_notice_to,
            Some(crate::messaging::model::DialogNoticeTo::Developer)
        );
        let messages = sent_by(&worker, room, agent);
        assert_eq!(messages.len(), 2);
        assert!(messages[1].0.is_empty());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn an_unreadable_deadline_observation_retains_help_until_an_open_snapshot() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let Some(crate::messaging::model::DialogNoticeTo::Orchestrator(notice)) =
        worker.state.agent(agent).unwrap().dialog_notice_to
    else {
        panic!("missing notice")
    };
    let deadline = worker.state.request(notice).unwrap().prompt.submitted_at_ms
        + crate::messaging::model::QUEUED_STALL_MS;
    worker.expire_queued_requests(deadline).unwrap();
    {
        let mut current = screen.lock().unwrap();
        current.answer_sent = true;
        current.settle_errors = 1;
    }
    worker.expire_unconfirmed_submissions(deadline).unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().dialog_notice_to,
        Some(crate::messaging::model::DialogNoticeTo::Orchestrator(
            notice
        ))
    );
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_some());
    assert_eq!(sent_by(&worker, room, agent).len(), 1);
    // The next ordinary tick can escalate once a coherent open snapshot returns.
    worker.expire_unconfirmed_submissions(deadline + 1).unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().dialog_notice_to,
        Some(crate::messaging::model::DialogNoticeTo::Developer)
    );
    worker
        .expire_unconfirmed_submissions(deadline + 60_000)
        .unwrap();
    let messages = sent_by(&worker, room, agent);
    assert_eq!(messages.len(), 2);
    assert!(!messages[1].1.contains("The dialog state could not be read"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_persistently_unreadable_open_dialog_still_gets_a_bounded_developer_fallback() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    screen.lock().unwrap().dialog = true;
    polls(&mut worker, 3);
    let Some(crate::messaging::model::DialogNoticeTo::Orchestrator(notice)) =
        worker.state.agent(agent).unwrap().dialog_notice_to
    else {
        panic!("the open dialog must have an orchestrator notice");
    };
    let deadline = worker.state.request(notice).unwrap().prompt.submitted_at_ms
        + crate::messaging::model::QUEUED_STALL_MS;
    {
        let mut current = screen.lock().unwrap();
        // AgentList continues to report this same open dialog. Detailed
        // observation remains unreadable, as during a sustained redraw.
        current.answer_sent = true;
        current.settle_errors = u32::MAX;
    }
    let grace = crate::messaging::coordinator::expiry::DIALOG_OBSERVATION_GRACE_MS;
    for elapsed in [
        0,
        crate::messaging::model::QUEUED_STALL_MS,
        grace - 1,
        grace,
        3_600_000,
        86_400_000,
    ] {
        let reloaded = worker.store.load().unwrap().unwrap();
        worker.save(reloaded).unwrap();
        worker.expire_queued_requests(deadline + elapsed).unwrap();
        worker.poll().unwrap();
        worker
            .expire_unconfirmed_submissions(deadline + elapsed)
            .unwrap();
        if elapsed < grace {
            assert_eq!(
                worker.state.agent(agent).unwrap().dialog_notice_to,
                Some(crate::messaging::model::DialogNoticeTo::Orchestrator(
                    notice
                ))
            );
            assert_eq!(sent_by(&worker, room, agent).len(), 1);
        } else {
            assert_eq!(
                worker.state.agent(agent).unwrap().dialog_notice_to,
                Some(crate::messaging::model::DialogNoticeTo::Developer)
            );
            assert_eq!(sent_by(&worker, room, agent).len(), 2);
        }
    }
    assert!(screen.lock().unwrap().current_dialog().is_some());
    assert!(worker.state.agent(agent).unwrap().dialog);
    let route = worker.state.agent(agent).unwrap().dialog_notice_to;
    let messages = sent_by(&worker, room, agent);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        route,
        Some(crate::messaging::model::DialogNoticeTo::Developer),
        "repeated unreadable observations must not suppress fallback forever, even a day past its bound"
    );
    assert_eq!(messages.len(), 2, "fallback must be posted exactly once");
    assert!(messages[1].1.contains("The dialog state could not be read"));
    assert!(messages[1].1.contains("may still be blocked"));
}

/// Stay clear of the clock-minute boundary so a same-minute check is stable.
fn settled_now() -> u64 {
    let now = crate::messaging::storage::io::now_ms();
    let into_minute = now % 60_000;
    if into_minute <= 55_000 {
        return now;
    }
    std::thread::sleep(std::time::Duration::from_millis(60_000 - into_minute + 50));
    crate::messaging::storage::io::now_ms()
}

fn queue_message(
    worker: &mut Worker,
    room: RoomId,
    recipient: AgentId,
    author: Author,
    text: &str,
    at_ms: u64,
) -> crate::messaging::model::RequestId {
    let mut state = worker.state.clone();
    let ids = state
        .submit_message_from(
            room,
            Draft {
                text: text.into(),
                files: Vec::new(),
                recipient_ids: [recipient].into(),
            },
            author,
            at_ms,
        )
        .unwrap();
    worker.save(state).unwrap();
    ids[0]
}

/// A completed answer whose task was submitted in the previous clock minute.
fn start_active_task(worker: &mut Worker, room: RoomId, agent: AgentId, text: &str, at_ms: u64) {
    let request = queue_message(worker, room, agent, Author::Developer, text, at_ms);
    let mut state = worker.state.clone();
    state.begin_submission(request, "launch", 0).unwrap();
    state
        .record_submission(
            request,
            crate::messaging::model::SubmissionOutcome::Confirmed {
                provider_session_id: None,
                provider_turn_id: None,
            },
        )
        .unwrap();
    worker.save(state).unwrap();
}

fn answer_previous_minute(worker: &mut Worker, room: RoomId, agent: AgentId, now: u64) {
    let request = queue_message(
        worker,
        room,
        agent,
        Author::Developer,
        "finish the job",
        now.saturating_sub(60_000),
    );
    let mut state = worker.state.clone();
    state.begin_submission(request, "launch", 0).unwrap();
    state
        .record_submission(
            request,
            crate::messaging::model::SubmissionOutcome::Confirmed {
                provider_session_id: None,
                provider_turn_id: None,
            },
        )
        .unwrap();
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(message, agent, "done".into(), now)
        .unwrap();
    worker.save(state).unwrap();
}

fn restart_log(at_ms: u64) -> crate::utils::restart_record::RestartLog {
    crate::utils::restart_record::RestartLog {
        at_ms,
        who: "test".into(),
        old_sha: "a".repeat(40),
        new_sha: "b".repeat(40),
        old_binary: "old".into(),
        new_binary: "new".into(),
        outcome: "restarted".into(),
        reason: String::new(),
    }
}

fn write_restart_marker(worker: &Worker) {
    crate::utils::restart_record::write_json(
        &crate::utils::restart_record::marker_path(&worker.data_dir),
        &crate::utils::restart_record::RestartMarker {
            binary: "bus".into(),
            session_id: None,
            explicit_data_dir: true,
            data_dir: worker.data_dir.display().to_string(),
            dev: true,
            yolo: false,
        },
    )
    .unwrap();
}

fn write_restart_outcome(worker: &Worker, at_ms: u64) {
    crate::utils::restart_record::write_json(
        &crate::utils::restart_record::outcome_path(&worker.data_dir),
        &restart_log(at_ms),
    )
    .unwrap();
}

fn record_restart_shaped_exit(worker: &mut Worker, agent: AgentId, at_ms: u64) {
    let mut state = worker.state.clone();
    state
        .record_exit_snapshot(
            agent,
            crate::messaging::model::ExitSnapshot {
                launch_id: Some("launch".into()),
                terminal_id: "terminal".into(),
                path: None,
                error: Some("pane closed during restart".into()),
                tail: "restart".into(),
                at_ms,
                terminal_closed: Some(false),
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    crate::utils::restart_record::append_log(
        &crate::utils::restart_record::log_path(&worker.data_dir),
        &restart_log(at_ms),
    )
    .unwrap();
}

fn open_dialog(screen: &std::sync::Arc<std::sync::Mutex<Screen>>) {
    screen.lock().unwrap().dialog = true;
}

#[test]
fn a_same_minute_task_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let now = settled_now();
    queue_message(
        &mut worker,
        room,
        agent,
        Author::Developer,
        "new task this minute",
        now,
    );
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "a task submitted to this agent in the same minute is not a dialog a person must answer"
    );
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_in_flight_same_minute_task_still_posts_one_steady_dialog() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    start_active_task(
        &mut worker,
        room,
        agent,
        "the turn that is showing the dialog",
        settled_now(),
    );
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(vec![orchestrator], expected_blocked(&worker, agent))]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_same_minute_result_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let now = settled_now();
    answer_previous_minute(&mut worker, room, agent, now);
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "a result completed this minute suppresses the notice even when the task is older"
    );
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_message_this_agent_sent_this_minute_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    let master = worker.state.master_room().unwrap().id;
    let now = settled_now();
    queue_message(
        &mut worker,
        master,
        orchestrator,
        Author::Agent(agent),
        "still sending",
        now,
    );
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "an agent still sending this minute is not blocked on a person"
    );
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_restart_in_progress_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    write_restart_marker(&worker);
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "a restart flap must not post Blocked"
    );
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_restart_outcome_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    write_restart_outcome(&worker, settled_now());
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "a recorded restart outcome must not post Blocked"
    );
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_restart_shaped_exit_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let now = settled_now();
    record_restart_shaped_exit(&mut worker, agent, now);
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "an exit in the same minute as a restart log is the flap, not a dialog"
    );
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_older_task_still_posts_one_steady_dialog() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    let now = settled_now();
    queue_message(
        &mut worker,
        room,
        agent,
        Author::Developer,
        "older task",
        now.saturating_sub(60_000),
    );
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(vec![orchestrator], expected_blocked(&worker, agent))]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_same_minute_task_for_someone_else_still_posts_the_dialog() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    let master = worker.state.master_room().unwrap().id;
    queue_message(
        &mut worker,
        master,
        orchestrator,
        Author::Developer,
        "task for the orchestrator",
        settled_now(),
    );
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(vec![orchestrator], expected_blocked(&worker, agent))]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_restart_log_from_another_minute_still_posts_one_steady_dialog() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    let now = settled_now();
    record_restart_shaped_exit(&mut worker, agent, now.saturating_sub(60_000));
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(vec![orchestrator], expected_blocked(&worker, agent))]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn removing_a_restart_marker_posts_one_steady_dialog() {
    let (mut worker, agent, room, orchestrator, screen, dir) = worker(true);
    let orchestrator = orchestrator.unwrap();
    write_restart_marker(&worker);
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "the flap itself posts nothing"
    );
    assert!(worker.state.agent(agent).unwrap().dialog_notice.is_none());
    std::fs::remove_file(crate::utils::restart_record::marker_path(&worker.data_dir)).unwrap();
    polls(&mut worker, 2);
    assert_eq!(
        sent_by(&worker, room, agent),
        [(vec![orchestrator], expected_blocked(&worker, agent))]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_bus_generated_result_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let now = settled_now();
    let mut state = worker.state.clone();
    let request = state
        .submit_bus_message_with(
            room,
            Draft {
                text: "initial task".into(),
                files: Vec::new(),
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            now,
            false,
        )
        .unwrap()[0];
    state.begin_submission(request, "launch", 0).unwrap();
    state
        .record_submission(
            request,
            crate::messaging::model::SubmissionOutcome::Confirmed {
                provider_session_id: None,
                provider_turn_id: None,
            },
        )
        .unwrap();
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(message, agent, "done".into(), now)
        .unwrap();
    worker.save(state).unwrap();
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "a real first-turn result must suppress Blocked even when Bus generated the request"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_task_quoting_the_blocked_sentence_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    queue_message(
        &mut worker,
        room,
        agent,
        Author::Developer,
        "Investigate why Bus reported: Blocked, needs help to continue.",
        settled_now(),
    );
    open_dialog(&screen);
    polls(&mut worker, 3);
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "quoting a notice must not hide a real new task from activity suppression"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_result_spooled_before_the_decisive_poll_suppresses_a_steady_dialog() {
    let (mut worker, agent, room, _, screen, dir) = worker(true);
    let now = settled_now();
    let request = queue_message(
        &mut worker,
        room,
        agent,
        Author::Developer,
        "finish before the next poll",
        now,
    );
    let mut state = worker.state.clone();
    state.begin_submission(request, "launch", 0).unwrap();
    state
        .record_submission(
            request,
            crate::messaging::model::SubmissionOutcome::Confirmed {
                provider_session_id: None,
                provider_turn_id: None,
            },
        )
        .unwrap();
    let prompt = state.render_request_payload(request).unwrap();
    worker.save(state).unwrap();
    let spool = worker.data_dir.join("callbacks").join("launch");
    crate::agents::providers::spool::initialize(
        &spool,
        &crate::agents::providers::spool::Manifest {
            routing_key: crate::agents::providers::spool::RoutingKey(agent.0),
            provider: Provider::ClaudeCode,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    crate::agents::providers::spool::append(
        &spool,
        "launch",
        Provider::ClaudeCode,
        json!({"hook_event_name":"UserPromptSubmit", "session_id":"session",
            "turn_id":"turn", "prompt_id":"turn", "prompt":prompt}),
        &[],
        false,
    )
    .unwrap();
    worker.consume_callbacks(agent, &spool).unwrap();
    open_dialog(&screen);
    worker.poll().unwrap();
    crate::agents::providers::spool::append(
        &spool,
        "launch",
        Provider::ClaudeCode,
        json!({"hook_event_name":"Stop", "session_id":"session", "turn_id":"turn",
            "prompt_id":"turn", "last_assistant_message":"done"}),
        &[],
        false,
    )
    .unwrap();
    worker.tick().unwrap();
    assert!(
        sent_by(&worker, room, agent).is_empty(),
        "a result already spooled before the decisive poll must not post Blocked first"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_stale_restart_marker_does_not_suppress_a_real_dialog_forever() {
    let (worker, agent, _, _, _, dir) = worker(true);
    write_restart_marker(&worker);
    let much_later = settled_now().saturating_add(86_400_000);
    assert!(
        !restart_suppresses(&worker, agent, much_later),
        "a marker left by an interrupted restart must not hide a real dialog a day later"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
