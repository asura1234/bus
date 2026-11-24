//! The chat attribution stays a bare sender line — same room, cross-room or
//! MASTER — while its `[Bus message N from …]` prefix names the sender's room.
//! Developer messages and legacy agents without a home room use the request room.
use std::path::PathBuf;

use super::*;

fn pair(room_name: &str) -> (BusState, RoomId, AgentId, AgentId) {
    let mut state = BusState::new();
    let room = state.create_room(room_name).unwrap();
    let author = state
        .create_agent(
            room,
            "builder",
            Provider::Codex,
            PathBuf::from("/repo"),
            None,
        )
        .unwrap();
    let peer = state
        .create_agent(
            room,
            "reviewer",
            Provider::ClaudeCode,
            PathBuf::from("/repo"),
            None,
        )
        .unwrap();
    (state, room, author, peer)
}

fn master_peer(state: &mut BusState) -> AgentId {
    let master = state.ensure_master_room();
    state
        .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
        .unwrap()
}

fn developer_request(state: &mut BusState, room: RoomId, peer: AgentId, text: &str) -> RequestId {
    state
        .submit_message_from(
            room,
            Draft {
                text: text.into(),
                files: Vec::new(),
                recipient_ids: [peer].into(),
            },
            Author::Developer,
            10,
        )
        .unwrap()[0]
}

fn delivered(
    state: &mut BusState,
    room: RoomId,
    author: AgentId,
    peer: AgentId,
    text: &str,
) -> (u64, String) {
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: text.into(),
                files: Vec::new(),
                recipient_ids: [peer].into(),
            },
            Author::Agent(author),
            11,
        )
        .unwrap()[0];
    let message = state.request(request).unwrap().prompt.id.0;
    let payload = state.render_request_payload(request).unwrap();
    (message, payload)
}

fn expect_message(message: u64, header: &str, prefix_from: &str, body: &str, payload: &str) {
    assert_eq!(
        payload,
        format!("{header}:\n[Bus message {message} from {prefix_from}]\n{body}"),
        "sender line or delivered prefix does not match the chat attribution format"
    );
}

#[test]
fn same_room_attribution_stays_bare_while_the_prefix_names_the_room() {
    let (mut state, room, author, peer) = pair("work");
    let (message, payload) = delivered(&mut state, room, author, peer, "please recheck");
    expect_message(
        message,
        "builder",
        &format!("builder (room {})", room.0),
        "please recheck",
        &payload,
    );
}

#[test]
fn cross_room_attribution_stays_bare_while_the_prefix_names_the_home_room() {
    let (mut state, home, author, _) = pair("home");
    let other = state.create_room("other").unwrap();
    let peer = state
        .create_agent(other, "peer", Provider::Codex, PathBuf::from("/repo"), None)
        .unwrap();
    let (message, payload) = delivered(&mut state, other, author, peer, "hello");
    expect_message(
        message,
        "builder",
        &format!("builder (room {})", home.0),
        "hello",
        &payload,
    );
}

#[test]
fn master_attribution_stays_bare_while_the_prefix_names_the_master_room() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let orch = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            PathBuf::from("/repo"),
            None,
        )
        .unwrap();
    let worker = state
        .create_agent(
            work,
            "builder",
            Provider::Codex,
            PathBuf::from("/repo"),
            None,
        )
        .unwrap();
    let (message, payload) = delivered(&mut state, work, orch, worker, "status");
    expect_message(
        message,
        "orch",
        &format!("orch (room {})", master.0),
        "status",
        &payload,
    );

    let peer = state
        .create_agent(
            master,
            "peer",
            Provider::Codex,
            PathBuf::from("/repo"),
            None,
        )
        .unwrap();
    let (message, payload) = delivered(&mut state, master, orch, peer, "in master");
    expect_message(
        message,
        "orch",
        &format!("orch (room {})", master.0),
        "in master",
        &payload,
    );
}

#[test]
fn numeric_and_renamed_senders_keep_the_id_and_the_prefix_keeps_the_room() {
    let (mut state, room, author, peer) = pair("work");
    let numeric = state
        .create_agent(room, "7", Provider::Codex, PathBuf::from("/repo"), None)
        .unwrap();
    let (message, payload) = delivered(&mut state, room, numeric, peer, "numeric sender");
    expect_message(
        message,
        &format!("7 (agent {})", numeric.0),
        &format!("7 (agent {}) (room {})", numeric.0, room.0),
        "numeric sender",
        &payload,
    );

    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "before rename".into(),
                files: Vec::new(),
                recipient_ids: [peer].into(),
            },
            Author::Agent(author),
            12,
        )
        .unwrap()[0];
    state.rename_agent(author, "renamed").unwrap();
    let message = state.request(request).unwrap().prompt.id.0;
    let payload = state.render_request_payload(request).unwrap();
    expect_message(
        message,
        &format!("builder (agent {})", author.0),
        &format!("builder (agent {}) (room {})", author.0, room.0),
        "before rename",
        &payload,
    );
}

#[test]
fn a_sender_with_no_home_room_uses_the_request_room() {
    let (mut state, room, author, peer) = pair("work");
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "legacy".into(),
                files: Vec::new(),
                recipient_ids: [peer].into(),
            },
            Author::Agent(author),
            11,
        )
        .unwrap()[0];
    state.delete_agent(author, 12).unwrap();
    let mut document = serde_json::to_value(&state).unwrap();
    document["requests"][request.0.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("sender_identity");
    document["retired_agents"]
        .as_object_mut()
        .unwrap()
        .remove(&author.0.to_string());
    let mut state: BusState = serde_json::from_value(document).unwrap();
    let message = state.request(request).unwrap().prompt.id.0;
    let payload = state.render_request_payload(request).unwrap();
    assert_eq!(
        payload,
        format!(
            "agent {}:\n[Bus message {message} from agent {} (room {})]\nlegacy",
            author.0, author.0, room.0
        )
    );
}

#[test]
fn a_developer_message_names_the_work_room() {
    let (mut state, room, _author, peer) = pair("work");
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "review the change".into(),
                files: Vec::new(),
                recipient_ids: [peer].into(),
            },
            Author::Developer,
            10,
        )
        .unwrap()[0];
    let message = state.request(request).unwrap().prompt.id.0;
    let payload = state.render_request_payload(request).unwrap();
    assert_eq!(
        payload,
        format!(
            "[Bus message {message} from you (room {})]\nreview the change",
            room.0
        )
    );
}

#[test]
fn a_developer_message_in_master_names_room_zero() {
    let mut state = BusState::new();
    let peer = master_peer(&mut state);
    let master = state.ensure_master_room();
    assert_eq!(master.0, 0);
    let request = developer_request(&mut state, master, peer, "status");
    let message = state.request(request).unwrap().prompt.id.0;
    assert_eq!(
        state.render_request_payload(request).unwrap(),
        format!("[Bus message {message} from you (room 0)]\nstatus")
    );
}

#[test]
fn a_developer_multi_recipient_message_keeps_names_after_the_room() {
    let (mut state, room, author, peer) = pair("work");
    let requests = state
        .submit_message_from(
            room,
            Draft {
                text: "shared task".into(),
                files: Vec::new(),
                recipient_ids: [peer, author].into(),
            },
            Author::Developer,
            10,
        )
        .unwrap();
    let message = state.request(requests[0]).unwrap().prompt.id.0;
    let expected = format!(
        "[Bus message {message} from you (room {}) to reviewer, builder]\nshared task",
        room.0
    );
    for request in requests {
        assert_eq!(state.render_request_payload(request).unwrap(), expected);
    }
}

fn check_restart_continuation_room(in_master: bool) {
    let (mut state, work, author, peer) = pair("work");
    let master = state.ensure_master_room();
    let orch = master_peer(&mut state);
    let (room, recipient) = if in_master {
        (master, orch)
    } else {
        (work, peer)
    };
    for (sender, name) in [
        (Author::Developer, "you"),
        (Author::Bus, "bus"),
        (Author::Agent(author), "builder"),
        (Author::Bot("watcher".into()), "watcher"),
    ] {
        let request = state
            .submit_message_from(
                room,
                Draft {
                    text: "original task".into(),
                    files: Vec::new(),
                    recipient_ids: [recipient].into(),
                },
                sender,
                10,
            )
            .unwrap()[0];
        let message = state.request(request).unwrap().prompt.id.0;
        let member = developer_request(&mut state, room, recipient, "joined task");
        let member_message = state.request(member).unwrap().prompt.id.0;
        state.requests.get_mut(&member).unwrap().group = Some(request);
        state
            .requests
            .get_mut(&request)
            .unwrap()
            .restart_continuation = true;
        assert_eq!(
            state.render_request_payload(request).unwrap(),
            format!(
                "[Bus message {message} from {name} (room {})]\nBus restarted or recovered your provider runtime while you were working on message {message} from {name} (room {}); your previous turn was interrupted. Check what you already did, then continue and finish that task. Messages {member_message} are still part of this task. Reply as usual to the original message IDs.",
                room.0, room.0
            )
        );
    }
}

#[test]
fn restart_continuation_names_master_room_zero() {
    check_restart_continuation_room(true);
}

#[test]
fn restart_continuation_names_the_work_room() {
    check_restart_continuation_room(false);
}
