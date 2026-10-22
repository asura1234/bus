use super::*;

fn dispatch(s: &mut BusState, room: RoomId, orch: AgentId, to: &[AgentId], text: &str) {
    s.submit_message_with(
        room,
        Draft {
            text: text.into(),
            files: vec![],
            recipient_ids: to.iter().copied().collect(),
        },
        Author::Agent(orch),
        90_000,
        true,
    )
    .unwrap();
}

fn ledger(s: &BusState, orch: AgentId) -> Value {
    serde_json::to_value(s).unwrap()["room_digests"][orch.0.to_string()].clone()
}

#[test]
fn room_digest_idle_alert_subjects_are_acknowledged_by_their_next_tasks() {
    let (mut s, room, orch, worker) = fixture();
    let second = s
        .create_agent(room, "second", Provider::Codex, "/repo".into(), None)
        .unwrap();
    for id in [worker, second] {
        s.observe_status(id, RuntimeStatus::Working, 10).unwrap();
        s.observe_status(id, RuntimeStatus::Idle, 20).unwrap();
    }
    s.notify_idle_agents(0, 80_000).unwrap();
    let notices: Vec<_> = facts(&s, orch)
        .into_iter()
        .filter(|f| {
            f["text"]
                .as_str()
                .unwrap()
                .contains("You have 2 idle agents")
        })
        .collect();
    assert_eq!(
        notices.len(),
        2,
        "the alert needs one fact per named worker"
    );
    assert!(notices.iter().any(|f| f["agent"] == json!(worker)));
    dispatch(&mut s, room, orch, &[worker], "next task");
    assert!(facts(&s, orch)
        .iter()
        .filter(|f| f["text"]
            .as_str()
            .unwrap()
            .contains("You have 2 idle agents"))
        .all(|f| f["agent"] == json!(second)));
    dispatch(&mut s, room, orch, &[second], "next task");
    assert!(
        facts(&s, orch).is_empty(),
        "retasking both workers handles the alert too"
    );
}

#[test]
fn room_digest_a_generic_notice_has_its_own_turn_without_a_duplicate_fact() {
    let (mut s, room, orch, _) = fixture();
    let notice = s
        .post_bus_notice(room, Some(orch), "GitHub: owned #1607 merged".into(), 10)
        .unwrap();
    assert!(s
        .requests()
        .any(|r| r.prompt.id == notice && r.prompt.author == Author::Bus));
    assert!(
        facts(&s, orch).is_empty(),
        "the notice already has its own delivery"
    );
    s.recover_room_digest_facts();
    assert!(
        facts(&s, orch).is_empty(),
        "recovery must agree with runtime"
    );
}

#[test]
fn room_digest_fact_text_is_capped_and_ack_keeps_only_a_minimal_tombstone() {
    let (mut s, room, orch, worker) = fixture();
    complete(
        &mut s,
        room,
        orch,
        worker,
        &format!("DONE {}", "界".repeat(2_000)),
    );
    let pending = facts(&s, orch);
    assert_eq!(pending[0]["text"].as_str().unwrap().chars().count(), 600);
    let id = pending[0]["id"].as_u64().unwrap();
    s.ack_digest_fact(id, 30).unwrap();
    let saved = ledger(&s, orch);
    assert!(saved["facts"].as_object().unwrap().is_empty());
    let tombstones = saved["tombstones"]
        .as_object()
        .expect("durable tombstone map");
    assert_eq!(tombstones.len(), 1);
    assert_eq!(
        tombstones.values().next().unwrap(),
        &json!({"id":id,"agent":worker,"acknowledged_at_ms":30})
    );
    let mut reopened: BusState = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    reopened.recover_room_digest_facts();
    assert!(
        facts(&reopened, orch).is_empty(),
        "retained final must not resurrect the acked payload"
    );
}

#[test]
fn room_digest_tombstones_expire_after_seven_days_without_rearming() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let id = facts(&s, orch)[0]["id"].as_u64().unwrap();
    s.ack_digest_fact(id, 30).unwrap();
    s.archive(30 + HISTORY_RETENTION_MS - 1);
    assert_eq!(
        ledger(&s, orch)["tombstones"]
            .as_object()
            .expect("tombstones")
            .len(),
        1
    );
    s.archive(30 + HISTORY_RETENTION_MS);
    assert!(ledger(&s, orch)["tombstones"]
        .as_object()
        .unwrap()
        .is_empty());
    assert!(!s.room_digest_armed(orch));
}

#[test]
fn room_digest_reference_ack_accepts_pr_name_and_head_but_not_partial_matches() {
    for (reference, handled) in [
        ("verify #1602", true),
        ("verify impl", true),
        ("verify at 8701126", true),
        ("verify #16020", false),
        ("verify implementation", false),
        ("verify at 8701127", false),
    ] {
        let (mut s, room, orch, worker) = fixture();
        complete(
            &mut s,
            room,
            orch,
            worker,
            "READY FOR VERIFY #1602\nhead 870112624",
        );
        let verifier = s
            .create_agent(room, "verifier", Provider::Codex, "/repo".into(), None)
            .unwrap();
        dispatch(&mut s, room, orch, &[verifier], reference);
        assert_eq!(facts(&s, orch).is_empty(), handled, "{reference}");
    }
}

#[test]
fn room_digest_multi_recipient_reference_is_not_an_action_ack() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let verifier = s
        .create_agent(room, "verifier", Provider::Codex, "/repo".into(), None)
        .unwrap();
    dispatch(&mut s, room, orch, &[worker, verifier], "verify #1602");
    assert!(facts(&s, orch).iter().any(|f| f["text"]
        .as_str()
        .is_some_and(|t| t.starts_with("READY"))
        && f["acknowledged_at_ms"].is_null()));
}

#[test]
fn room_digest_a_tagged_completion_has_no_second_idle_fact_on_later_observation() {
    let (mut s, room, orch, worker) = fixture();
    s.observe_status(worker, RuntimeStatus::Working, 5).unwrap();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    s.observe_status(worker, RuntimeStatus::Idle, 25).unwrap();
    assert_eq!(
        facts(&s, orch).len(),
        1,
        "same completion is one actionable fact"
    );
}

#[test]
fn room_digest_watermark_advances_only_on_delivery_and_restores_on_abandonment() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let source = s.requests().find(|r| r.agent_id == worker).unwrap().id;
    s.see_worker_reply(source, orch, 20).unwrap();
    s.observe_status(orch, RuntimeStatus::Idle, 20).unwrap();
    s.set_agent_runtime_identity(
        orch,
        AgentRuntimeIdentity {
            launch_id: Some("orch".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(s.queue_room_digest(orch, 30).unwrap());
    assert_eq!(
        ledger(&s, orch)["watermark"],
        json!(0),
        "queued is not delivered"
    );
    let digest = s.next_queued_request(orch).unwrap();
    s.begin_submission(digest, "orch", 0).unwrap();
    s.record_submission(
        digest,
        SubmissionOutcome::Confirmed {
            provider_session_id: None,
            provider_turn_id: None,
        },
    )
    .unwrap();
    assert!(ledger(&s, orch)["watermark"].as_u64().unwrap() > 0);
    s.recover_idle_request(digest, 40).unwrap();
    assert_eq!(ledger(&s, orch)["watermark"], json!(0));
    assert!(s.queue_room_digest(orch, 50).unwrap());
}

fn queued_digest() -> (BusState, AgentId, RequestId) {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let source = s.requests().find(|r| r.agent_id == worker).unwrap().id;
    s.see_worker_reply(source, orch, 20).unwrap();
    s.observe_status(orch, RuntimeStatus::Idle, 20).unwrap();
    s.set_agent_runtime_identity(
        orch,
        AgentRuntimeIdentity {
            launch_id: Some("orch".into()),
            ..Default::default()
        },
    )
    .unwrap();
    s.queue_room_digest(orch, 30).unwrap();
    let digest = s.next_queued_request(orch).unwrap();
    (s, orch, digest)
}

#[test]
fn room_digest_recovery_does_not_recreate_sources_below_a_delivered_watermark() {
    let (mut s, orch, digest) = queued_digest();
    s.begin_submission(digest, "orch", 0).unwrap();
    s.record_submission(
        digest,
        SubmissionOutcome::Confirmed {
            provider_session_id: None,
            provider_turn_id: None,
        },
    )
    .unwrap();
    s.finish_for_test(digest, "noted", 40);
    let watermark = ledger(&s, orch)["watermark"].clone();
    let mut saved = serde_json::to_value(&s).unwrap();
    saved["room_digests"][orch.0.to_string()]["facts"] = json!({});
    let mut recovered: BusState = serde_json::from_value(saved).unwrap();
    recovered.recover_room_digest_facts();
    assert!(
        facts(&recovered, orch).is_empty(),
        "retained completed requests are already covered"
    );
    assert_eq!(ledger(&recovered, orch)["watermark"], watermark);
    assert!(!recovered.room_digest_armed(orch));
}

#[test]
fn room_digest_rejected_and_uncertain_pastes_survive_restart_without_delivery_credit() {
    for outcome in [
        SubmissionOutcome::DefinitelyRejected {
            message: "pane busy".into(),
        },
        SubmissionOutcome::Uncertain {
            message: "submit timed out".into(),
        },
    ] {
        let (mut s, orch, digest) = queued_digest();
        s.begin_submission(digest, "orch", 0).unwrap();
        s.record_submission(digest, outcome).unwrap();
        let mut recovered: BusState =
            serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        recovered.recover_room_digest_facts();
        assert_eq!(ledger(&recovered, orch)["watermark"], json!(0));
        assert!(ledger(&recovered, orch)["pending_watermark"]
            .as_u64()
            .is_some());
        recovered.recover_idle_request(digest, 40).unwrap();
        assert!(recovered.queue_room_digest(orch, 50).unwrap());
    }
}

#[test]
fn room_digest_expired_tombstone_sources_cannot_return_after_history_replay() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let id = facts(&s, orch)[0]["id"].as_u64().unwrap();
    s.ack_digest_fact(id, 30).unwrap();
    s.prune_digest_tombstones(30 + HISTORY_RETENTION_MS);
    assert!(s
        .requests()
        .any(|r| r.agent_id == worker && r.phase == RequestPhase::Completed));
    s.recover_room_digest_facts();
    assert!(
        facts(&s, orch).is_empty(),
        "expired dedupe must not resurrect retained acknowledged history"
    );
    assert!(!s.room_digest_armed(orch));
}

#[test]
fn room_digest_legacy_recovery_obeys_reference_and_broadcast_action_rules() {
    for broadcast in [false, true] {
        let (mut s, room, orch, worker) = fixture();
        complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
        s.room_digests.clear();
        let verifier = s
            .create_agent(room, "verifier", Provider::Codex, "/repo".into(), None)
            .unwrap();
        s.submit_message_with_broadcast(
            room,
            Draft {
                text: "verify #1602".into(),
                files: vec![],
                recipient_ids: [verifier].into(),
            },
            Author::Agent(orch),
            30,
            true,
            broadcast,
        )
        .unwrap();
        s.recover_room_digest_facts();
        assert_eq!(
            facts(&s, orch).is_empty(),
            !broadcast,
            "only direct references count after upgrade"
        );
    }
}

#[test]
fn room_digest_a_later_real_idle_episode_after_a_tagged_completion_is_new_work() {
    let (mut s, room, orch, worker) = fixture();
    s.observe_status(worker, RuntimeStatus::Working, 5).unwrap();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    s.observe_status(worker, RuntimeStatus::Idle, 25).unwrap();
    let verifier = s
        .create_agent(room, "verifier", Provider::Codex, "/repo".into(), None)
        .unwrap();
    dispatch(&mut s, room, orch, &[verifier], "verify #1602");
    assert!(facts(&s, orch).is_empty());
    s.observe_status(worker, RuntimeStatus::Working, 90_010)
        .unwrap();
    s.observe_status(worker, RuntimeStatus::Idle, 90_020)
        .unwrap();
    assert!(facts(&s, orch)
        .iter()
        .any(|f| f["text"] == "Idle with no next task"));
}

#[test]
fn room_digest_upgrade_migrates_old_payloads_and_notice_keys_without_replay() {
    let (mut s, room, orch, worker) = fixture();
    complete(&mut s, room, orch, worker, "READY FOR VERIFY #1602");
    let ready = facts(&s, orch)[0].clone();
    s.post_developer_attention(worker, &format!("Worker alert {}", "界".repeat(1_000)), 40)
        .unwrap();
    let generic = s
        .post_bus_notice(room, Some(orch), "Old generic notice".into(), 45)
        .unwrap();
    s.set_agent_runtime_identity(
        orch,
        AgentRuntimeIdentity {
            launch_id: Some("orch".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let request = s.requests().find(|r| r.prompt.id == generic).unwrap().id;
    s.begin_submission(request, "orch", 0).unwrap();
    s.record_submission(
        request,
        SubmissionOutcome::Confirmed {
            provider_session_id: None,
            provider_turn_id: None,
        },
    )
    .unwrap();
    s.finish_for_test(request, "handled", 46);
    let generic_fact_id = s.allocate_id();
    let mut saved = serde_json::to_value(&s).unwrap();
    let record = &mut saved["room_digests"][orch.0.to_string()];
    let mut old = serde_json::Map::new();
    let mut generic_fact = None;
    for (mut key, mut fact) in record["facts"].as_object().unwrap().clone() {
        fact.as_object_mut().unwrap().remove("source_id");
        if fact["id"] == ready["id"] {
            fact["acknowledged_at_ms"] = json!(30);
        } else {
            key = key.rsplit_once(':').unwrap().0.to_owned();
            fact["text"] = json!(format!("Worker alert {}", "界".repeat(1_000)));
            generic_fact = Some(fact.clone());
        }
        old.insert(key, fact);
    }
    let mut generic_fact = generic_fact.unwrap();
    generic_fact["id"] = json!(generic_fact_id);
    generic_fact["agent"] = Value::Null;
    generic_fact["agent_name"] = json!("bus");
    generic_fact["at_ms"] = json!(45);
    generic_fact["text"] = json!("Old generic notice");
    old.insert(format!("notice:{}", generic.0), generic_fact);
    record["facts"] = Value::Object(old);
    for field in [
        "pending_watermark",
        "previous_watermark",
        "tombstones",
        "pruned_through",
    ] {
        record.as_object_mut().unwrap().remove(field);
    }
    let mut reopened: BusState = serde_json::from_value(saved).unwrap();
    reopened.recover_room_digest_facts();
    let pending = facts(&reopened, orch);
    assert_eq!(
        pending.len(),
        1,
        "old notice keys must dedupe their canonical runtime source"
    );
    assert_eq!(pending[0]["agent"], json!(worker));
    assert_eq!(pending[0]["text"].as_str().unwrap().chars().count(), 600);
    let migrated = ledger(&reopened, orch);
    let tombstones = migrated["tombstones"].as_object().unwrap();
    assert!(tombstones.values().any(|t| t["id"] == ready["id"]
        && t["acknowledged_at_ms"] == json!(30)
        && t.get("text").is_none()));
    assert!(
        tombstones
            .values()
            .any(|t| t["id"] == json!(generic_fact_id)),
        "a previously delivered generic notice must not wake again"
    );
}

#[test]
fn room_digest_a_pr_the_final_only_mentions_in_passing_does_not_ack_its_ready() {
    let (mut s, room, orch, worker) = fixture();
    complete(
        &mut s,
        room,
        orch,
        worker,
        "READY FOR VERIFY #1602\nRebased onto #138's messaging rules; head 870112624.",
    );
    let other = s
        .create_agent(room, "docs", Provider::Codex, "/repo".into(), None)
        .unwrap();
    dispatch(
        &mut s,
        room,
        orch,
        &[other],
        "Read the #138 messaging rules before you start.",
    );
    assert!(
        facts(&s, orch).iter().any(
            |f| f["text"].as_str().is_some_and(|t| t.starts_with("READY"))
                && f["acknowledged_at_ms"].is_null()
        ),
        "an unrelated brief naming #138 cleared the #1602 READY"
    );
}

#[test]
fn room_digest_reference_keys_belong_to_the_tag_or_explicit_head_field() {
    for (identity_suffix, message, handled) in [
        ("\nDependency abcdef123456", "Read abcdef123", false),
        (" 20261023", "Inspect 20261023", false),
        ("\nBuild 870112624", "Inspect 8701126", false),
        (" abcdef123456", "Verify abcdef123", true),
        (" head 870112624", "Verify at 8701126", true),
        ("\nSHA: 870112624", "Verify at 8701126", true),
        ("\nhead abcdef123456", "Verify abcdef123", true),
        (
            "\nhead abcdef123456 of a dependency",
            "Read abcdef123",
            false,
        ),
        ("\nhead", "Read abcdef123", false),
        ("\nRebased onto head 870112624", "Read 8701126", false),
        ("", "Rebase now; #1602 is waiting for a verifier", true),
    ] {
        let (mut s, room, orch, worker) = fixture();
        let final_text = format!("READY FOR VERIFY #1602{identity_suffix}");
        complete(&mut s, room, orch, worker, &final_text);
        let verifier = s
            .create_agent(room, "verifier", Provider::Codex, "/repo".into(), None)
            .unwrap();
        dispatch(&mut s, room, orch, &[verifier], message);
        assert_eq!(
            facts(&s, orch).is_empty(),
            handled,
            "{final_text:?}: {message}"
        );
    }
}

#[test]
fn room_digest_a_dependency_head_sentence_does_not_supply_milestone_identity() {
    let (mut s, room, orch, worker) = fixture();
    complete(
        &mut s,
        room,
        orch,
        worker,
        "READY FOR VERIFY #1602\nHead of the dependency branch is SHA abcdef123456.",
    );
    let other = s
        .create_agent(room, "docs", Provider::Codex, "/repo".into(), None)
        .unwrap();
    dispatch(&mut s, room, orch, &[other], "Read abcdef123");
    assert!(
        facts(&s, orch).iter().any(|fact| fact["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("READY"))),
        "a dependency's SHA in ordinary prose cleared the #1602 READY"
    );
}
