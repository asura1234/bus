//! The chat attribution stays a bare sender line — same room, cross-room or
//! MASTER — while its `[Bus message N from …]` prefix names the sender's room.
//! A sender with no room keeps the bare label; nothing invents one.
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
fn a_sender_with_no_room_keeps_the_bare_label() {
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
            "agent {}:\n[Bus message {message} from agent {}]\nlegacy",
            author.0, author.0
        )
    );
    assert!(
        !payload.contains("(room"),
        "a missing room must not be invented: {payload}"
    );
}

#[test]
fn a_developer_message_has_no_room_to_name() {
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
        format!("[Bus message {message} from you]\nreview the change")
    );
}
