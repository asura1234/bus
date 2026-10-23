use super::*;
use serde_json::{json, Value};

#[path = "room_digest_round_one_test.rs"]
mod round_one;

fn fixture() -> (BusState, RoomId, AgentId, AgentId) {
    let mut s = BusState::new();
    let master = s.ensure_master_room();
    let room = s.create_room("digest-work").unwrap();
    let orch = s
        .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
        .unwrap();
    s.bind_orchestrator(orch, room).unwrap();
    let worker = s
        .create_agent(room, "impl", Provider::Codex, "/repo".into(), None)
        .unwrap();
    s.confirm_hook_setup(worker).unwrap();
    s.set_agent_runtime_identity(
        worker,
        AgentRuntimeIdentity {
            launch_id: Some("launch".into()),
            ..Default::default()
        },
    )
    .unwrap();
    (s, room, orch, worker)
}

fn facts(s: &BusState, orch: AgentId) -> Vec<Value> {
    serde_json::to_value(s).unwrap()["room_digests"][orch.0.to_string()]["facts"]
        .as_object()
        .map(|v| v.values().cloned().collect())
        .unwrap_or_default()
}

fn complete(s: &mut BusState, room: RoomId, orch: AgentId, worker: AgentId, text: &str) {
    let request = s
        .submit_message_from(
            room,
            Draft {
                text: "work".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Agent(orch),
            10,
        )
        .unwrap()[0];
    s.begin_submission(request, "launch", 0).unwrap();
    s.record_submission(
        request,
        SubmissionOutcome::Confirmed {
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn".into()),
        },
    )
    .unwrap();
    s.requests.get_mut(&request).unwrap().pending_final = Some(PendingFinal {
        callback_id: "final".into(),
        text: text.into(),
        received_at_ms: 20,
        provider_session_id: Some("session".into()),
        provider_turn_id: Some("turn".into()),
    });
    s.complete_pending_final(worker, 20).unwrap();
}

#[test]
fn room_digest_captures_status_tags_only_at_the_start_of_the_first_reply_line() {
    for tag in [
        "READY FOR VERIFY",
        "MERGED",
        "DONE",
        "BLOCKED",
        "REVIEW READY",
    ] {
        let (mut s, room, orch, worker) = fixture();
        let text = format!("{tag} #1602\nhead abc123");
        complete(&mut s, room, orch, worker, &text);
        let retained = facts(&s, orch);
        assert_eq!(retained.len(), 1, "{tag}: {retained:?}");
        assert_eq!(retained[0]["text"], text);
        assert_eq!(retained[0]["agent_name"], "impl");
        assert_eq!(retained[0]["acknowledged_at_ms"], Value::Null);
    }
    for text in [
        "Still working\nDONE later",
        "The word BLOCKED is an example",
        "DONEISH",
    ] {
        let (mut s, room, orch, worker) = fixture();
        complete(&mut s, room, orch, worker, text);
        assert!(facts(&s, orch).is_empty(), "not a status-tag final: {text}");
    }
}

#[test]
fn room_digest_retains_a_worker_idle_episode_without_a_next_task_once() {
    let (mut s, room, orch, worker) = fixture();
    s.observe_status(worker, RuntimeStatus::Working, 10)
        .unwrap();
    s.observe_status(worker, RuntimeStatus::Idle, 20).unwrap();
    let first = facts(&s, orch);
    assert_eq!(first.len(), 1);
    assert!(first[0]["text"]
        .as_str()
        .unwrap()
        .contains("Idle with no next task"));
    for now in [21, 3_600_001, 7_200_001] {
        s.observe_status(worker, RuntimeStatus::Idle, now).unwrap();
    }
    assert_eq!(facts(&s, orch), first, "idle time is not a new fact");
    s.submit_message_from(
        room,
        Draft {
            text: "next".into(),
            files: vec![],
            recipient_ids: [worker].into(),
        },
        Author::Agent(orch),
        7_200_002,
    )
    .unwrap();
    assert!(facts(&s, orch)
        .iter()
        .all(|v| v["acknowledged_at_ms"].is_u64()));
}

#[test]
fn room_digest_a_new_idle_worker_without_an_assignment_is_actionable() {
    let (mut s, _, orch, worker) = fixture();
    s.observe_status(worker, RuntimeStatus::Idle, 10).unwrap();
    assert_eq!(
        facts(&s, orch).len(),
        1,
        "newly available workers also need an owner"
    );
}

#[test]
fn room_digest_generic_github_and_disk_notices_keep_their_own_delivery() {
    use crate::messaging::github::{Page, PullRequest, Scope, Update};
    let (mut s, room, orch, _) = fixture();
    let update = Update::Repo {
        repo: "o/r".into(),
        scopes: vec![Scope {
            room: room.0,
            branches: ["fix/ours".into()].into(),
        }],
        result: Ok(Page {
            default_branch: "main".into(),
            merged: vec![PullRequest {
                number: 1607,
                branch: "fix/ours".into(),
                base: "main".into(),
                merged_at_ms: Some(1000),
                ..Default::default()
            }],
            ..Default::default()
        }),
        at_ms: 2000,
    };
    s.apply_github(&update, 2000).unwrap();
    assert!(s
        .requests()
        .any(|r| r.agent_id == orch && r.prompt.text.contains("#1607")));
    assert!(
        facts(&s, orch).is_empty(),
        "standalone notice is not a duplicate action fact"
    );
    let before = facts(&s, orch).len();
    s.apply_github(&update, 3000).unwrap();
    assert_eq!(facts(&s, orch).len(), before);
    s.notify_disk_space(1, 20, 5, 4000).unwrap();
    assert!(s
        .requests()
        .any(|r| r.agent_id == orch && r.prompt.text.contains("URGENT")));
    assert!(facts(&s, orch).is_empty());
    let saved: BusState = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(facts(&saved, orch), facts(&s, orch));
}

#[test]
fn room_digest_keeps_abandoned_and_stalled_requests_independently_of_seen_routes() {
    for abandoned in [true, false] {
        let (mut s, room, orch, worker) = fixture();
        s.observe_status(worker, RuntimeStatus::Idle, 1).unwrap();
        let request = s
            .submit_message_from(
                room,
                Draft {
                    text: "do work".into(),
                    files: vec![],
                    recipient_ids: [worker].into(),
                },
                Author::Agent(orch),
                10,
            )
            .unwrap()[0];
        if abandoned {
            s.recover_idle_request(request, 20).unwrap();
        }
        s.retain_worker_outcomes(QUEUED_STALL_MS + 100);
        let expected = if abandoned { "abandoned" } else { "stalled" };
        assert!(
            facts(&s, orch)
                .iter()
                .any(|v| v["text"].as_str().unwrap().contains(expected)),
            "{expected}"
        );
    }
}

#[test]
fn room_digest_a_developer_message_cannot_ack_the_orchestrators_fact() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "DONE #143");
    s.submit_message_from(
        room,
        Draft {
            text: "developer question".into(),
            files: vec![],
            recipient_ids: [worker].into(),
        },
        Author::Developer,
        30,
    )
    .unwrap();
    assert_eq!(facts(&s, orch).len(), 1);
    assert_eq!(facts(&s, orch)[0]["acknowledged_at_ms"], json!(null));
}

#[test]
fn room_digest_recovery_does_not_duplicate_a_retained_peer_final() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY peer work");
    let peer = s
        .create_agent(room, "peer", Provider::Codex, "/repo".into(), None)
        .unwrap();
    let completed = s
        .requests
        .values_mut()
        .find(|r| r.agent_id == worker)
        .unwrap();
    completed.prompt.author = Author::Agent(peer);
    s.retain_worker_outcomes(100);
    let before = facts(&s, orch);
    s.recover_room_digest_facts();
    assert_eq!(
        facts(&s, orch),
        before,
        "one final must not re-arm through its compatibility event after restart"
    );
}

#[test]
fn room_digest_recovery_reconciles_a_legacy_followup_action() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY old work");
    s.room_digests.clear(); // State written before digest support.
    s.submit_message_from(
        room,
        Draft {
            text: "verification assigned; do next task".into(),
            files: vec![],
            recipient_ids: [worker].into(),
        },
        Author::Agent(orch),
        30,
    )
    .unwrap();
    s.recover_room_digest_facts();
    assert!(
        facts(&s, orch).is_empty(),
        "an upgrade must not resurrect already handled work"
    );
    let saved = serde_json::to_value(&s).unwrap();
    let tombstones = saved["room_digests"][orch.0.to_string()]["tombstones"]
        .as_object()
        .unwrap();
    assert_eq!(tombstones.len(), 1);
    assert_eq!(
        tombstones.values().next().unwrap()["acknowledged_at_ms"],
        json!(30)
    );
}

#[test]
fn room_digest_cross_room_final_also_reaches_the_conversation_orchestrator() {
    let (mut s, room, orch, _) = fixture();
    let remote = s.create_room("other").unwrap();
    let worker = s
        .create_agent(remote, "remote", Provider::Codex, "/repo".into(), None)
        .unwrap();
    s.set_agent_runtime_identity(
        worker,
        AgentRuntimeIdentity {
            launch_id: Some("launch".into()),
            ..Default::default()
        },
    )
    .unwrap();
    complete(
        &mut s,
        room,
        orch,
        worker,
        "READY FOR VERIFY cross-room dispatch",
    );
    assert!(
        facts(&s, orch).iter().any(|f| f["agent"] == json!(worker)),
        "the dispatch owner must see the handoff even when the worker lives elsewhere"
    );
}

#[test]
fn room_digest_cross_room_abandoned_and_stalled_requests_reach_the_dispatch_owner() {
    for abandoned in [true, false] {
        let (mut s, room, orch, _) = fixture();
        let remote = s.create_room("other").unwrap();
        let worker = s
            .create_agent(remote, "remote", Provider::Codex, "/repo".into(), None)
            .unwrap();
        s.confirm_hook_setup(worker).unwrap();
        s.observe_status(worker, RuntimeStatus::Idle, 1).unwrap();
        let request = s
            .submit_message_from(
                room,
                Draft {
                    text: "cross-room work".into(),
                    files: vec![],
                    recipient_ids: [worker].into(),
                },
                Author::Agent(orch),
                10,
            )
            .unwrap()[0];
        if abandoned {
            s.recover_idle_request(request, 20).unwrap();
        } else {
            s.notify_stalled_attention(QUEUED_STALL_MS + 100).unwrap();
        }
        assert!(
            facts(&s, orch).iter().any(|f| f["agent"] == json!(worker)),
            "abandoned={abandoned}"
        );
    }
}

#[test]
fn room_digest_only_new_facts_rearm_and_a_burst_coalesces_with_older_unacknowledged_work() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    s.observe_status(orch, RuntimeStatus::Idle, 20).unwrap();
    assert!(
        !s.queue_room_digest(orch, 20).unwrap(),
        "the pending actual worker reply takes priority"
    );
    let source = s.requests().find(|r| r.agent_id == worker).unwrap().id;
    s.see_worker_reply(source, orch, 20).unwrap();
    assert!(s.queue_room_digest(orch, 20).unwrap());
    let digest = s
        .requests()
        .find(|r| r.prompt.text.starts_with("Room digest:"))
        .unwrap()
        .id;
    s.set_agent_runtime_identity(
        orch,
        AgentRuntimeIdentity {
            launch_id: Some("orch".into()),
            ..Default::default()
        },
    )
    .unwrap();
    s.begin_submission(digest, "orch", 0).unwrap();
    s.record_submission(
        digest,
        SubmissionOutcome::Confirmed {
            provider_session_id: None,
            provider_turn_id: None,
        },
    )
    .unwrap();
    s.finish_for_test(digest, "noted", 21);
    assert!(
        !s.queue_room_digest(orch, 3_600_020).unwrap(),
        "time alone cannot re-arm"
    );
    s.record_digest_final(
        worker,
        room,
        super::digests::DigestSource::new("new:merged".into(), worker.0),
        "MERGED #1607",
        3_600_020,
    );
    s.record_digest_final(
        worker,
        room,
        super::digests::DigestSource::new("new:blocked".into(), worker.0),
        "BLOCKED next item",
        3_600_021,
    );
    assert!(s.queue_room_digest(orch, 3_600_022).unwrap());
    let text = &s
        .requests()
        .filter(|r| r.prompt.text.starts_with("Room digest:"))
        .last()
        .unwrap()
        .prompt
        .text;
    for expected in [
        "READY FOR VERIFY #1602",
        "MERGED #1607",
        "BLOCKED next item",
        "age 3600s",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert_eq!(
        s.requests()
            .filter(|r| r.prompt.text.starts_with("Room digest:"))
            .count(),
        2
    );
    assert!(
        s.expire_stalled_queued_requests(7_200_022)
            .unwrap()
            .iter()
            .all(|id| !s
                .request(*id)
                .unwrap()
                .prompt
                .text
                .starts_with("Room digest:")),
        "an undelivered digest must survive ordinary queued expiry"
    );
}

#[test]
fn room_digest_a_healthy_background_idle_notice_is_not_actionable() {
    use super::reply_routing::ReplyRoute;
    use super::worker_events::{WorkerEvent, WorkerEventKind};
    let (mut s, room, orch, worker) = fixture();
    let request = s
        .submit_message_from(
            room,
            Draft {
                text: "healthy background task".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Agent(orch),
            10,
        )
        .unwrap()[0];
    s.begin_submission(request, "launch", 0).unwrap();
    s.record_submission(
        request,
        SubmissionOutcome::Confirmed {
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn".into()),
        },
    )
    .unwrap();
    s.requests.get_mut(&request).unwrap().awaiting_background = true;
    s.observe_status(worker, RuntimeStatus::Idle, 20).unwrap();
    let event = WorkerEvent {
        agent: worker,
        room,
        group: Some(request),
        message: s.request(request).unwrap().prompt.id,
        at_ms: 20,
        episode: Some(0),
        kind: WorkerEventKind::Idle,
        text: "Idle with open request (background work pending)".into(),
        route: ReplyRoute::Seen,
    };
    s.record_digest_event("idle:healthy:0", &event);
    assert!(
        facts(&s, orch).is_empty(),
        "healthy work already has an owner and a live wait; do not create an action digest"
    );
}

#[test]
fn room_digest_restart_does_not_turn_a_delivered_bus_dialog_notice_into_a_new_wake() {
    // The coordinator's dialog notice to an orchestrator is a plain Bus-authored
    // message (coordinator/dialogs.rs) and records no fact at runtime.
    let (mut s, room, orch, _) = fixture();
    s.submit_message_from(
        room,
        Draft {
            text: "impl is showing a dialog: choose an option".into(),
            files: vec![],
            recipient_ids: [orch].into(),
        },
        Author::Bus,
        10,
    )
    .unwrap();
    let before = facts(&s, orch);
    let armed = s.room_digest_armed(orch);
    s.recover_room_digest_facts();
    assert_eq!(
        facts(&s, orch),
        before,
        "a restart must not create facts the running Bus never recorded"
    );
    assert_eq!(
        s.room_digest_armed(orch),
        armed,
        "a restart alone must not re-arm"
    );
}

#[test]
fn room_digest_status_tag_followed_by_sentence_punctuation_is_captured() {
    for text in ["DONE.", "BLOCKED.\nwaiting on creds", "MERGED, #1607"] {
        let (mut s, room, orch, worker) = fixture();
        complete(&mut s, room, orch, worker, text);
        assert_eq!(
            facts(&s, orch).len(),
            1,
            "first line starts with a tag: {text:?}"
        );
    }
}

#[test]
fn room_digest_an_abandoned_digest_leaves_its_unacknowledged_facts_deliverable() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    s.observe_status(orch, RuntimeStatus::Idle, 20).unwrap();
    let source = s.requests().find(|r| r.agent_id == worker).unwrap().id;
    s.see_worker_reply(source, orch, 20).unwrap();
    assert!(s.queue_room_digest(orch, 20).unwrap());
    let digest = s
        .requests()
        .find(|r| r.prompt.text.starts_with("Room digest:"))
        .unwrap()
        .id;
    s.recover_idle_request(digest, 30).unwrap();
    assert_eq!(s.request(digest).unwrap().phase, RequestPhase::Abandoned);
    assert!(
        s.queue_room_digest(orch, 40).unwrap(),
        "the READY fact was never shown; an abandoned digest must not swallow it"
    );
}

#[test]
fn staffing_deep_or_old_queues_create_one_actionable_fact_per_backlog() {
    for (depth, now) in [(4, 100), (1, 900_101)] {
        let (mut s, room, orch, worker) = fixture();
        for _ in 0..depth {
            s.submit_message_with(
                room,
                Draft {
                    text: "queued work".into(),
                    files: vec![],
                    recipient_ids: [worker].into(),
                },
                Author::Developer,
                100,
                true,
            )
            .unwrap();
        }
        s.observe_status(worker, RuntimeStatus::Working, now)
            .unwrap();
        let pressure: Vec<_> = facts(&s, orch)
            .into_iter()
            .filter(|f| f["text"].as_str().unwrap().contains("Queue pressure"))
            .collect();
        assert_eq!(pressure.len(), 1, "{pressure:?}");
        s.observe_status(worker, RuntimeStatus::Working, now + 1)
            .unwrap();
        assert_eq!(facts(&s, orch).len(), 1);
    }
}

#[test]
fn staffing_provider_capacity_failure_is_a_blocked_digest_fact() {
    let (mut s, room, orch, worker) = fixture();
    let request = s
        .submit_message_from(
            room,
            Draft {
                text: "work".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Developer,
            100,
        )
        .unwrap()[0];
    s.begin_submission(request, "launch", 0).unwrap();
    s.close_turn_with_reason(
        request,
        200,
        "provider_error: model at capacity (server_overloaded)".into(),
    )
    .unwrap();
    assert!(
        facts(&s, orch)
            .iter()
            .any(|f| f["text"].as_str().unwrap().starts_with("BLOCKED")
                && f["text"].as_str().unwrap().contains("server_overloaded")),
        "{:?}",
        facts(&s, orch)
    );
}

#[test]
fn staffing_queue_pressure_has_strict_thresholds_and_covers_the_orchestrator() {
    let (mut s, room, orch, worker) = fixture();
    for _ in 0..3 {
        s.submit_message_with(
            room,
            Draft {
                text: "queued".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Developer,
            100,
            true,
        )
        .unwrap();
    }
    s.observe_status(worker, RuntimeStatus::Working, 900_100)
        .unwrap();
    assert!(
        facts(&s, orch).is_empty(),
        "exactly three messages and fifteen minutes is below the threshold"
    );
    s.observe_status(worker, RuntimeStatus::Working, 900_101)
        .unwrap();
    assert_eq!(facts(&s, orch).len(), 1);
    for _ in 0..4 {
        s.submit_message_with(
            room,
            Draft {
                text: "hub work".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Developer,
            900_101,
            true,
        )
        .unwrap();
    }
    s.observe_status(orch, RuntimeStatus::Working, 900_102)
        .unwrap();
    assert!(facts(&s, orch)
        .iter()
        .any(|f| f["agent"] == orch.0 && f["text"].as_str().unwrap().contains("Queue pressure")));
}

#[test]
fn queue_pressure_records_one_fact_while_a_deep_backlog_drains() {
    let (mut s, room, orch, worker) = fixture();
    for _ in 0..8 {
        s.submit_message_with(
            room,
            Draft {
                text: "queued work".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Developer,
            100,
            true,
        )
        .unwrap();
    }
    let mut now = 1_000;
    for _ in 0..4 {
        let request = s.queued_requests(worker)[0];
        s.begin_submission(request, "launch", 0).unwrap();
        s.record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
        s.observe_status(worker, RuntimeStatus::Working, now)
            .unwrap();
        s.requests.get_mut(&request).unwrap().pending_final = Some(PendingFinal {
            callback_id: format!("final-{now}"),
            text: "done".into(),
            received_at_ms: now + 1,
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn".into()),
        });
        s.observe_status(worker, RuntimeStatus::Idle, now + 2)
            .unwrap();
        now += 10;
    }
    let pressure: Vec<_> = facts(&s, orch)
        .into_iter()
        .filter(|f| f["text"].as_str().unwrap().contains("Queue pressure"))
        .collect();
    assert_eq!(
        pressure.len(),
        1,
        "one backlog produced {} pressure facts: {pressure:?}",
        pressure.len()
    );
}

#[test]
fn queue_pressure_updates_without_rearming_and_restarts_only_after_a_healthy_queue() {
    fn drain(s: &mut BusState, worker: AgentId, at: u64) {
        let request = s.queued_requests(worker)[0];
        s.begin_submission(request, "launch", 0).unwrap();
        s.record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
        s.requests.get_mut(&request).unwrap().pending_final = Some(PendingFinal {
            callback_id: format!("processed-{at}"),
            text: "processed".into(),
            received_at_ms: at,
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn".into()),
        });
        s.complete_pending_final(worker, at).unwrap();
    }
    let (mut s, room, orch, worker) = fixture();
    let enqueue = |s: &mut BusState, at| {
        s.submit_message_with(
            room,
            Draft {
                text: "queued work".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Developer,
            at,
            true,
        )
        .unwrap();
    };
    let pressure = |s: &BusState| {
        facts(s, orch)
            .into_iter()
            .filter(|f| f["text"].as_str().unwrap().starts_with("Queue pressure"))
            .collect::<Vec<_>>()
    };
    for _ in 0..6 {
        enqueue(&mut s, 100);
    }
    let first = pressure(&s)[0].clone();
    let fact_id = first["id"].as_u64().unwrap();
    s.observe_status(orch, RuntimeStatus::Idle, 200).unwrap();
    assert!(s.queue_room_digest(orch, 200).unwrap());
    assert!(!s.room_digest_armed(orch));
    drain(&mut s, worker, 300);
    s = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    s.observe_status(worker, RuntimeStatus::Working, 10_000)
        .unwrap();
    let updated = pressure(&s);
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0]["id"], first["id"]);
    assert_eq!(updated[0]["at_ms"], first["at_ms"]);
    assert!(updated[0]["text"]
        .as_str()
        .unwrap()
        .contains("5 queued messages; oldest 9s"));
    assert!(
        !s.room_digest_armed(orch),
        "updating the backlog must not wake its owner again"
    );
    s.ack_digest_fact(fact_id, 10_001).unwrap();
    s = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    s.recover_room_digest_facts();
    s.observe_status(worker, RuntimeStatus::Working, 10_002)
        .unwrap();
    assert!(
        pressure(&s).is_empty(),
        "an acknowledged episode must survive restart without resurfacing"
    );
    while s.queued_requests(worker).len() > 3 {
        drain(&mut s, worker, 20_000);
    }
    enqueue(&mut s, 30_000);
    let next = pressure(&s);
    assert_eq!(next.len(), 1);
    assert_ne!(
        next[0]["id"], first["id"],
        "crossing the threshold after recovery starts a new episode"
    );
    assert!(s.room_digest_armed(orch));
}
