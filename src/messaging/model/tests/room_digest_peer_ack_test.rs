use super::*;

fn reviewer(s: &mut BusState, room: RoomId) -> AgentId {
    s.create_agent(room, "reviewer", Provider::Codex, "/repo".into(), None)
        .unwrap()
}

fn brief(s: &mut BusState, room: RoomId, orch: AgentId, peer: AgentId, text: &str) {
    s.submit_message_with(
        room,
        Draft {
            text: text.into(),
            files: vec![],
            recipient_ids: [peer].into(),
        },
        Author::Agent(orch),
        30,
        true,
    )
    .unwrap();
}

#[test]
fn room_digest_peer_brief_matches_a_multiline_pr_field() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "REVIEW READY\nPR: #1602");
    let peer = reviewer(&mut s, room);
    brief(&mut s, room, orch, peer, "Review #1602");
    assert!(
        facts(&s, orch).is_empty(),
        "the explicit PR field did not identify the READY fact"
    );
}

#[test]
fn room_digest_peer_brief_matches_a_backticked_head_field() {
    let (mut s, room, orch, worker) = fixture();
    complete(
        &mut s,
        room,
        orch,
        worker,
        "READY FOR VERIFY\nHead: `abcdef123456789`",
    );
    let peer = reviewer(&mut s, room);
    brief(&mut s, room, orch, peer, "Verify abcdef1");
    assert!(
        facts(&s, orch).is_empty(),
        "a 7-character head prefix should identify the fact"
    );
}

#[test]
fn room_digest_peer_brief_about_a_different_pr_does_not_ack_even_when_it_names_the_worker() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let peer = reviewer(&mut s, room);
    brief(&mut s, room, orch, peer, "Review impl's #1603");
    assert_eq!(
        facts(&s, orch).len(),
        1,
        "the worker name overrode the different PR"
    );
}

#[test]
fn room_digest_peer_brief_must_be_sent_in_the_fact_room() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let other = s.create_room("other").unwrap();
    let peer = reviewer(&mut s, other);
    brief(&mut s, other, orch, peer, "Review #1602");
    assert_eq!(
        facts(&s, orch).len(),
        1,
        "a brief in another room retired this room's fact"
    );
}

#[test]
fn room_digest_recovered_peer_brief_uses_the_same_multiline_identity_match() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "REVIEW READY\nPR: #1602");
    s.room_digests.clear();
    let peer = reviewer(&mut s, room);
    brief(&mut s, room, orch, peer, "Review #1602");
    s.recover_room_digest_facts();
    assert!(
        facts(&s, orch).is_empty(),
        "recovery resurrected an already assigned READY fact"
    );
}

#[test]
fn room_digest_recovered_peer_brief_in_another_room_does_not_ack() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    s.room_digests.clear();
    let other = s.create_room("other").unwrap();
    let peer = reviewer(&mut s, other);
    brief(&mut s, other, orch, peer, "Review #1602");
    s.recover_room_digest_facts();
    assert_eq!(
        facts(&s, orch).len(),
        1,
        "replay credited another room's brief"
    );
}

#[test]
fn room_digest_peer_brief_does_not_match_a_longer_pr_token() {
    for text in ["Review #16020", "Review #1602suffix"] {
        let (mut s, room, orch, worker) = fixture();
        complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
        let peer = reviewer(&mut s, room);
        brief(&mut s, room, orch, peer, text);
        assert_eq!(facts(&s, orch).len(), 1, "{text} was treated as #1602");
    }
}

#[test]
fn room_digest_peer_ack_keeps_direct_worker_assignments_and_dependency_exclusions() {
    let (mut s, room, orch, worker) = fixture();
    complete(
        &mut s,
        room,
        orch,
        worker,
        "READY FOR VERIFY\nPR: #1602\nRebased onto #138 and SHA abcdef123456.",
    );
    let peer = reviewer(&mut s, room);
    brief(&mut s, room, orch, peer, "Read #138 and abcdef123");
    assert_eq!(
        facts(&s, orch).len(),
        1,
        "dependency prose identified the fact"
    );
    brief(&mut s, room, orch, worker, "Next task");
    assert!(
        facts(&s, orch).is_empty(),
        "direct worker assignment stopped acknowledging"
    );
}

#[test]
fn room_digest_peer_brief_accepts_explicit_markdown_identity_fields() {
    for field in [
        "- **PR**: [#1602](https://github.com/o/r/pull/1602)",
        "PR [#1602](https://github.com/o/r/pull/1602)",
        "SHA: [abcdef123456](https://github.com/o/r/commit/abcdef123456)",
        "HEAD `abcdef123456`",
    ] {
        let (mut s, room, orch, worker) = fixture();
        complete(
            &mut s,
            room,
            orch,
            worker,
            &format!("REVIEW READY\n{field}"),
        );
        let peer = reviewer(&mut s, room);
        brief(&mut s, room, orch, peer, "Verify #1602 at abcdef1");
        assert!(facts(&s, orch).is_empty(), "{field}");
    }
}

#[test]
fn room_digest_peer_brief_accepts_a_numeric_prefix_of_a_known_head() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "REVIEW READY 1234567abcdef12");
    let peer = reviewer(&mut s, room);
    brief(&mut s, room, orch, peer, "Verify 1234567");
    assert!(facts(&s, orch).is_empty());
}

#[test]
fn room_digest_peer_brief_can_target_a_remote_agent_in_the_fact_conversation() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "REVIEW READY #1602");
    let other = s.create_room("other").unwrap();
    let peer = reviewer(&mut s, other);
    brief(&mut s, room, orch, peer, "Review #1602");
    assert!(
        facts(&s, orch).is_empty(),
        "recipient home is not the conversation room"
    );
}

#[test]
fn room_digest_peer_brief_rejects_short_and_embedded_identity_tokens() {
    for text in [
        "Review #1602_extra",
        "Verify abcdef",
        "Verify abcdef1_extra",
    ] {
        let (mut s, room, orch, worker) = fixture();
        complete(
            &mut s,
            room,
            orch,
            worker,
            "READY FOR VERIFY #1602\nHead: abcdef123456789",
        );
        let peer = reviewer(&mut s, room);
        brief(&mut s, room, orch, peer, text);
        assert_eq!(facts(&s, orch).len(), 1, "{text}");
    }
}

#[test]
fn room_digest_direct_worker_assignment_from_master_still_acknowledges() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let master = s.ensure_master_room();
    brief(&mut s, master, orch, worker, "Next task from MASTER");
    assert!(
        facts(&s, orch).is_empty(),
        "#145 direct-worker action must remain cross-room"
    );
}

#[test]
fn room_digest_recovered_direct_worker_assignment_from_master_still_acknowledges() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    s.room_digests.clear();
    let master = s.ensure_master_room();
    brief(&mut s, master, orch, worker, "Next task from MASTER");
    s.recover_room_digest_facts();
    assert!(
        facts(&s, orch).is_empty(),
        "replay must preserve cross-room direct-worker action"
    );
}
