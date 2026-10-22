use super::*;

fn add(worker: &mut Worker, room: RoomId, dir: &std::path::Path, name: &str) -> AgentId {
    worker
        .state
        .create_agent(room, name, Provider::Codex, dir.into(), None)
        .unwrap()
}

fn send(worker: &mut Worker, room: RoomId, author: AgentId, to: &str, text: &str) -> Response {
    static NEXT_SEND: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let key = format!(
        "open-{}",
        NEXT_SEND.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    call(
        worker,
        &key,
        "message.send",
        json!({"room":room.0.to_string(),"as":author.0.to_string(),"to":[to],"text":text}),
    )
}

#[test]
fn open_messaging_reaches_other_workers_orchestrators_and_master_agents() {
    let (mut w, room, author, dir) = fixture();
    let other_room = w.state.create_room("other").unwrap();
    let master = w.state.master_room().unwrap().id;
    let peer = add(&mut w, other_room, &dir, "foreign-worker");
    let orch = add(&mut w, master, &dir, "foreign-orch");
    w.state.bind_orchestrator(orch, other_room).unwrap();
    let master_agent = add(&mut w, master, &dir, "master-peer");
    for recipient in [peer, orch, master_agent] {
        for selector in [
            recipient.0.to_string(),
            w.state.agent(recipient).unwrap().name.clone(),
        ] {
            let response = send(&mut w, room, author, &selector, "cross-room question");
            assert!(response.ok, "{response:?}");
            let request = w.state.requests().last().unwrap();
            assert_eq!(request.agent_id, recipient);
            assert_eq!(request.room_id, room);
            assert_eq!(request.prompt.author, Author::Agent(author));
        }
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn open_messaging_ambiguous_global_names_list_candidates() {
    let (mut w, room, author, dir) = fixture();
    let other = w.state.create_room("other").unwrap();
    let master = w.state.master_room().unwrap().id;
    let a = add(&mut w, other, &dir, "duplicate");
    let b = add(&mut w, master, &dir, "duplicate");
    let response = send(&mut w, room, author, "duplicate", "question");
    let error = error_message(&response);
    assert!(error.contains("Ambiguous"), "{error}");
    for (id, room) in [(a, other), (b, master)] {
        assert!(
            error.contains(&format!("duplicate (agent {}, room {})", id.0, room.0)),
            "{error}"
        );
    }
    assert_eq!(w.state.requests().count(), 0);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn open_messaging_local_name_wins_but_an_exact_id_wins_over_a_numeric_name() {
    let (mut w, room, author, dir) = fixture();
    let other = w.state.create_room("other").unwrap();
    let remote = add(&mut w, other, &dir, "same");
    let local = add(&mut w, room, &dir, "same");
    add(&mut w, room, &dir, &remote.0.to_string());
    assert!(send(&mut w, room, author, "same", "local").ok);
    assert_eq!(w.state.requests().last().unwrap().agent_id, local);
    assert!(send(&mut w, room, author, &remote.0.to_string(), "id").ok);
    assert_eq!(w.state.requests().last().unwrap().agent_id, remote);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn open_messaging_all_agent_payloads_name_the_sender_and_its_home_room() {
    let (mut w, room, author, dir) = fixture();
    let peer = add(&mut w, room, &dir, "peer");
    let master = w.state.master_room().unwrap().id;
    // The conversation's room need not be the sender's home room.
    assert!(send(&mut w, master, author, &peer.0.to_string(), "hello").ok);
    let id = w.state.requests().last().unwrap().id;
    let expected = format!("codex1 (agent {}, room {}):\nhello", author.0, room.0);
    assert_eq!(
        w.state.render_request_payload(id).as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(w.state.request(id).unwrap().prompt.text, "hello");
    let mut restored: BusState =
        serde_json::from_value(serde_json::to_value(&w.state).unwrap()).unwrap();
    restored.rename_agent(author, "renamed").unwrap();
    assert_eq!(
        restored.render_request_payload(id).as_deref(),
        Some(expected.as_str())
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn open_messaging_does_not_duplicate_an_existing_sender_identity() {
    for opening in [
        "codex1 here —",
        "From codex1:",
        "codex1",
        "codex1 (agent 3, room 2):",
        "Agent 3:",
        "agent 3 here —",
    ] {
        let (mut w, room, author, dir) = fixture();
        let master = w.state.master_room().unwrap().id;
        let peer = add(&mut w, master, &dir, "own-orch");
        w.state.bind_orchestrator(peer, room).unwrap();
        let opening = opening
            .replace("agent 3", &format!("agent {}", author.0))
            .replace("Agent 3", &format!("Agent {}", author.0));
        let text = format!("{opening}\nready for verification");
        assert!(send(&mut w, room, author, &peer.0.to_string(), &text).ok);
        let id = w.state.requests().last().unwrap().id;
        assert_eq!(
            w.state.render_request_payload(id).as_deref(),
            Some(text.as_str())
        );
        drop(w);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn open_messaging_a_different_or_embedded_identity_does_not_hide_the_sender() {
    for text in [
        "codex10 here —\nhello",
        "From codex10:\nhello",
        "agent 30:\nhello",
        "Ready; codex1 reviewed this",
        "hello\ncodex1",
    ] {
        let (mut w, room, author, dir) = fixture();
        let peer = add(&mut w, room, &dir, "peer");
        assert!(send(&mut w, room, author, &peer.0.to_string(), text).ok);
        let id = w.state.requests().last().unwrap().id;
        assert_eq!(
            w.state.render_request_payload(id).unwrap(),
            format!("codex1 (agent {}, room {}):\n{text}", author.0, room.0)
        );
        drop(w);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn open_messaging_alias_and_names_follow_the_senders_home_room() {
    let (mut w, home, author, dir) = fixture();
    let conversation = w.state.create_room("conversation").unwrap();
    let master = w.state.master_room().unwrap().id;
    let orch = add(&mut w, master, &dir, "own-orch");
    w.state.bind_orchestrator(orch, home).unwrap();
    let peer = add(&mut w, home, &dir, "peer");
    add(&mut w, conversation, &dir, "peer");
    assert!(send(&mut w, conversation, author, "ORCHESTRATOR", "blocker").ok);
    assert_eq!(w.state.requests().last().unwrap().agent_id, orch);
    assert!(send(&mut w, conversation, author, "peer", "home name").ok);
    assert_eq!(w.state.requests().last().unwrap().agent_id, peer);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn open_messaging_queued_attribution_survives_sender_rename_and_deletion() {
    let (mut w, home, author, dir) = fixture();
    let other = w.state.create_room("other").unwrap();
    let peer = add(&mut w, other, &dir, "peer");
    assert!(
        send(
            &mut w,
            other,
            author,
            &peer.0.to_string(),
            "queued question"
        )
        .ok
    );
    let id = w.state.requests().last().unwrap().id;
    w.state.rename_agent(author, "renamed").unwrap();
    w.state.delete_agent(author, 5).unwrap();
    let mut restored: BusState =
        serde_json::from_value(serde_json::to_value(&w.state).unwrap()).unwrap();
    assert_eq!(
        restored.render_request_payload(id).unwrap(),
        format!(
            "codex1 (agent {}, room {}):\nqueued question",
            author.0, home.0
        )
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

fn deletion_preserves_foreign_recipient(active: bool) {
    let (mut w, home, author, dir) = fixture();
    let other = w.state.create_room("other").unwrap();
    let peer = add(&mut w, other, &dir, "peer");
    assert!(
        send(
            &mut w,
            home,
            author,
            &peer.0.to_string(),
            "discard this room"
        )
        .ok
    );
    let removed = w.state.requests().last().unwrap().id;
    let kept = w
        .state
        .submit_message_from(
            other,
            crate::messaging::model::Draft {
                text: "keep this room".into(),
                files: vec![],
                recipient_ids: [peer].into(),
            },
            Author::Developer,
            1,
        )
        .unwrap()[0];
    if active {
        w.state
            .set_agent_runtime_identity(
                peer,
                AgentRuntimeIdentity {
                    launch_id: Some("foreign".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        w.state.begin_submission(removed, "foreign", 0).unwrap();
        w.state
            .record_submission(
                removed,
                SubmissionOutcome::Confirmed {
                    provider_session_id: None,
                    provider_turn_id: None,
                },
            )
            .unwrap();
    }
    w.state.delete_room(home, 10).unwrap();
    assert!(w.state.agent(peer).is_some());
    assert!(w.state.request(removed).is_none());
    assert!(w.state.request(kept).is_some());
    assert_eq!(w.state.agent(peer).unwrap().current_request, None);
    assert_eq!(w.state.queued_requests(peer), &[kept]);
    assert_eq!(w.state.next_queued_request(peer), Some(kept));
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn open_messaging_room_deletion_clears_foreign_queued_request_references() {
    deletion_preserves_foreign_recipient(false);
}

#[test]
fn open_messaging_room_deletion_clears_foreign_active_request_references() {
    deletion_preserves_foreign_recipient(true);
}

#[test]
fn open_messaging_an_existing_header_matches_names_that_begin_with_from() {
    for name in ["From worker", "From agent 9", "Équipe"] {
        let (mut w, room, author, dir) = fixture();
        w.state.rename_agent(author, name).unwrap();
        let peer = add(&mut w, room, &dir, "peer");
        let text = format!("{name} (agent {}, room {}):\nready", author.0, room.0);
        assert!(send(&mut w, room, author, &peer.0.to_string(), &text).ok);
        let id = w.state.requests().last().unwrap().id;
        assert_eq!(
            w.state.render_request_payload(id).as_deref(),
            Some(text.as_str())
        );
        drop(w);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
