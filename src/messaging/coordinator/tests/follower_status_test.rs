use super::*;
use crate::messaging::{
    control::{self, ASYNC_SEND_FOLLOWER},
    coordinator::{BusCommand, Draft, Provider},
    model::{AgentRuntimeIdentity, RequestId},
};
use std::{sync::mpsc, time::Duration};

fn fixture() -> (BusState, RequestId, AgentId, AgentId) {
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let author = state
        .create_agent(room, "sender", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    let recipient = state
        .create_agent(room, "recipient", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            recipient,
            AgentRuntimeIdentity {
                launch_id: Some("fixture-launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "build".into(),
                recipient_ids: [recipient].into(),
                ..Default::default()
            },
            Author::Agent(author),
            crate::messaging::storage::io::now_ms(),
        )
        .unwrap()[0];
    (state, request, author, recipient)
}

fn view(state: BusState, waiters: &ReplyWaiters) -> FollowerStatus {
    FollowerStatus {
        snapshots: Arc::new(Mutex::new(Arc::new(BusSnapshot {
            state,
            revision: 1,
            last_command_id: 0,
            error: None,
        }))),
        waiters: Arc::downgrade(waiters),
    }
}

fn status(message: PromptId, selector: &str) -> Request {
    Request {
        id: "follower".into(),
        method: "message.status".into(),
        params: json!({"message":message.0.to_string(),"as":selector}),
    }
}

#[test]
fn marked_follower_gets_pending_status_while_an_older_command_occupies_the_queue() {
    let (mut state, request, author, _) = fixture();
    state
        .begin_submission(request, "fixture-launch", 0)
        .unwrap();
    let record = state.request(request).unwrap();
    let (message, room) = (record.prompt.id, record.room_id);
    let waiters = ReplyWaiters::default();
    let view = view(state, &waiters);
    let mut marked = status(message, &author.0.to_string());
    marked.params[ASYNC_SEND_FOLLOWER] = json!(true);
    let path = crate::utils::test_temp::unique_temp_path("bfs");
    let (commands, receiver) = mpsc::sync_channel(1);
    commands
        .send((24, BusCommand::SetDraftText(room, "held".into())))
        .unwrap();
    let server =
        control::server::start_with_follower_status(&path, commands, view.clone()).unwrap();
    let reply = control::request_with_timeout(&path, &marked, Duration::from_secs(3));
    let ready = waiters.lock().unwrap().contains_key(&message);
    let command = receiver.try_recv().unwrap();
    let ordinary = status(message, &author.0.to_string());
    let call_path = path.clone();
    let ordinary_call = std::thread::spawn(move || control::request(&call_path, &ordinary));
    let (_, BusCommand::Control(call)) = receiver.recv_timeout(Duration::from_secs(3)).unwrap()
    else {
        panic!("ordinary status must dispatch through the coordinator");
    };
    assert_eq!(call.request.method, "message.status");
    call.reply
        .send(Response::success(
            &call.request.id,
            json!({"complete": true}),
        ))
        .unwrap();
    assert_eq!(
        ordinary_call.join().unwrap().unwrap().result["complete"],
        true
    );
    drop(server);
    std::fs::remove_dir_all(path).unwrap();
    let reply = reply.expect("the status hint must answer before its deadline");
    assert_eq!(reply.result["snapshot_pending"], true);
    assert_eq!(reply.result["complete"], false);
    assert_eq!(reply.result["requests"], json!([]));
    assert!(
        ready,
        "the author lease must be visible to reply routing before the hint"
    );
    assert!(matches!(command, (24, BusCommand::SetDraftText(..))));
}

#[test]
fn follower_snapshot_defers_live_status_and_rejects_an_invalid_waiter() {
    let (mut state, request, author, recipient) = fixture();
    let id = state.request(request).unwrap().prompt.id;
    let waiters = ReplyWaiters::default();
    assert!(view(state.clone(), &waiters)
        .response(&status(id, "sender"))
        .is_none());
    state
        .begin_submission(request, "fixture-launch", 0)
        .unwrap();
    let ready = view(state.clone(), &waiters);
    assert!(ready.response(&status(id, "recipient")).is_none());
    assert!(ready
        .response(&status(PromptId(u64::MAX), "sender"))
        .is_none());
    assert!(waiters.lock().unwrap().is_empty());
    assert!(ready.response(&status(id, "sender")).is_some());
    assert!(ready.response(&status(id, &author.0.to_string())).is_some());

    let mut dialog = state.clone();
    dialog.observe_dialog(recipient, true, 1).unwrap();
    assert!(view(dialog, &waiters)
        .response(&status(id, "sender"))
        .is_none());
    state.finish_for_test(request, "done", 2);
    assert!(view(state, &waiters)
        .response(&status(id, "sender"))
        .is_none());
    drop(waiters);
    assert!(ready.response(&status(id, "sender")).is_none());
}
