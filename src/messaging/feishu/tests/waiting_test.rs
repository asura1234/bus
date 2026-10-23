use super::*;
use crate::messaging::model::{AgentId, Author, Provider, RoomId};

fn fixture() -> (BusState, RoomId, RoomId, AgentId) {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("libtv-app").unwrap();
    let agent = state
        .create_agent(
            master,
            "libtv-app master",
            Provider::ClaudeCode,
            "/tmp".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, work).unwrap();
    (state, master, work, agent)
}

fn enabled() -> WaitingNotifications {
    let mut policy = WaitingNotifications::default();
    policy.configure(true);
    policy
}

#[test]
fn developer_notification_to_human_has_only_sender_and_first_line() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(
        master,
        agent,
        "Summary\n[waiting-on-you]\n- Need Figma access\nPRIVATE second line".into(),
        vec![],
        1,
    )
    .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    assert!(policy.take(now + 1000).is_none());
    assert_eq!(
        policy.take(now + 2000).as_deref(),
        Some("libtv-app master is waiting on you: - Need Figma access")
    );
    assert!(policy.take(now + 4000).is_none());
    policy.observe(&next, &next, now + 5000);
    assert!(policy.take(now + 8000).is_none());
}

#[test]
fn developer_notification_final_reply_to_developer_triggers_once() {
    let (mut before, master, _, agent) = fixture();
    before.set_draft_text(master, "status").unwrap();
    before.set_draft_recipients(master, [agent]).unwrap();
    let request = before.submit_draft(master, 1).unwrap()[0];
    let mut value = serde_json::to_value(&before).unwrap();
    let item = &mut value["requests"][request.0.to_string()];
    item["phase"] = serde_json::json!("completed");
    item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"Still waiting on you: Approve export", "received_at_ms":2, "provider_session_id":null, "provider_turn_id":null});
    item["completed_at_ms"] = serde_json::json!(2);
    let next: BusState = serde_json::from_value(value).unwrap();
    let mut policy = enabled();
    let now = 1000_u64;
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + 2000).as_deref(),
        Some("libtv-app master is waiting on you: Approve export")
    );
    policy.observe(&next, &next, now + 3000);
    assert!(policy.take(now + 6000).is_none());
}

#[test]
fn developer_notification_coalesces_messages_without_extending_window() {
    let (before, master, _, agent) = fixture();
    let mut first = before.clone();
    first
        .post_to_developer(master, agent, "[waiting-on-you] first".into(), vec![], 1)
        .unwrap();
    let mut second = first.clone();
    second
        .post_to_developer(master, agent, "waiting on you: second".into(), vec![], 2)
        .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &first, now);
    policy.observe(&first, &second, now + 1900);
    assert_eq!(
        policy.take(now + 2000).as_deref(),
        Some(
            "libtv-app master is waiting on you: first\nlibtv-app master is waiting on you: second"
        )
    );
    assert!(policy.take(now + 5000).is_none());
}

#[test]
fn developer_notification_toggle_discards_pending_without_replay() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(master, agent, "[waiting-on-you] hello".into(), vec![], 1)
        .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    policy.configure(false);
    policy.configure(true);
    assert!(policy.take(now + 3000).is_none());
    policy.observe(&next, &next, now + 4000);
    assert!(policy.take(now + 7000).is_none());
    policy.configure(false);
    policy.observe(&before, &next, now);
    assert!(policy.take(now + 3000).is_none());
}

#[test]
fn developer_notification_includes_work_rooms_and_tagged_agent_directed_prompts() {
    let (before, master, work, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(work, agent, "[waiting-on-you] work room".into(), vec![], 1)
        .unwrap();
    next.submit_message_from(
        master,
        crate::messaging::model::Draft {
            text: "[waiting-on-you] agent directed".into(),
            files: vec![],
            recipient_ids: [agent].into(),
        },
        Author::Agent(agent),
        2,
    )
    .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    assert_eq!(policy.take(now + 3000).as_deref(), Some("libtv-app master is waiting on you: work room\nlibtv-app master is waiting on you: agent directed"));
}

#[test]
fn developer_notification_full_action_is_unicode_and_control_free() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(
        master,
        agent,
        format!("[waiting-on-you] {}\u{1b}\u{7}\nPRIVATE", "界".repeat(130)),
        vec![],
        1,
    )
    .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    let text = policy.take(now + 2000).unwrap();
    assert_eq!(text.split_once(": ").unwrap().1, "界".repeat(130));
    assert!(!text.chars().any(char::is_control));
    assert!(!text.contains("PRIVATE"));
}

#[test]
fn developer_notification_carriage_return_ends_the_preview() {
    assert_eq!(plain_line("first\rPRIVATE"), "first");
}

#[test]
fn developer_notification_archive_moves_do_not_replay_notices() {
    let (mut before, master, _, agent) = fixture();
    before
        .post_to_developer(master, agent, "[waiting-on-you] old".into(), vec![], 1)
        .unwrap();
    let mut archived = before.clone();
    assert!(archived.archive(20 * 60 * 1000));
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &archived, now);
    assert!(policy.take(now + 3000).is_none());
}

#[test]
fn developer_notification_coalesced_request_group_is_one_final() {
    let (mut before, master, _, agent) = fixture();
    before.set_draft_recipients(master, [agent]).unwrap();
    before.set_draft_text(master, "first").unwrap();
    let first = before.submit_draft(master, 1).unwrap()[0];
    before.set_draft_text(master, "second").unwrap();
    let second = before.submit_draft(master, 2).unwrap()[0];
    let mut value = serde_json::to_value(&before).unwrap();
    for request in [first, second] {
        let item = &mut value["requests"][request.0.to_string()];
        item["phase"] = serde_json::json!("completed");
        item["group"] = if request == first {
            serde_json::Value::Null
        } else {
            serde_json::json!(first.0)
        };
        item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"[waiting-on-you] one answer", "received_at_ms":3, "provider_session_id":null, "provider_turn_id":null});
        item["completed_at_ms"] = serde_json::json!(3);
    }
    let next: BusState = serde_json::from_value(value).unwrap();
    let mut policy = enabled();
    let now = 1000_u64;
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + 3000).as_deref(),
        Some("libtv-app master is waiting on you: one answer")
    );
}

#[test]
fn developer_notification_typed_bus_attention_uses_identified_sender() {
    let (before, _, _, agent) = fixture();
    let mut next = before.clone();
    next.post_developer_attention(agent, "[waiting-on-you] Process exited", 1)
        .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    let text = policy.take(now + 3000).unwrap();
    assert!(text.starts_with("libtv-app master is waiting on you: "));
    assert!(text.contains("Process exited"));
}

#[test]
fn developer_notification_attention_preview_omits_internal_tag() {
    let (before, master, _, agent) = fixture();
    let mut bus_notice = before.clone();
    bus_notice
        .post_developer_attention(agent, "[waiting-on-you] Process exited", 1)
        .unwrap();
    let mut tagged_post = before.clone();
    tagged_post
        .post_to_developer(
            master,
            agent,
            format!(
                "{} [waiting-on-you] Need Figma access",
                crate::messaging::attention::TAG
            ),
            vec![],
            1,
        )
        .unwrap();
    let now = 1000_u64;
    for (next, expected) in [
        (
            bus_notice,
            "libtv-app master is waiting on you: Process exited",
        ),
        (
            tagged_post,
            "libtv-app master is waiting on you: Need Figma access",
        ),
    ] {
        let mut policy = enabled();
        policy.observe(&before, &next, now);
        assert_eq!(policy.take(now + 3000).as_deref(), Some(expected));
    }
}

#[test]
fn developer_notification_tagged_final_uses_the_visible_first_line() {
    let (mut before, master, _, agent) = fixture();
    before.set_draft_text(master, "status").unwrap();
    before.set_draft_recipients(master, [agent]).unwrap();
    let request = before.submit_draft(master, 1).unwrap()[0];
    let mut value = serde_json::to_value(&before).unwrap();
    let item = &mut value["requests"][request.0.to_string()];
    item["phase"] = serde_json::json!("completed");
    item["pending_final"] = serde_json::json!({"callback_id":"done", "text":format!("{}\n[waiting-on-you]\nNeed Figma access\nPRIVATE", crate::messaging::attention::TAG), "received_at_ms":2, "provider_session_id":null, "provider_turn_id":null});
    item["completed_at_ms"] = serde_json::json!(2);
    let next: BusState = serde_json::from_value(value).unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + 3000).as_deref(),
        Some("libtv-app master is waiting on you: Need Figma access")
    );
}

#[test]
fn developer_notification_attention_marker_preserves_full_action() {
    let (before, _, _, agent) = fixture();
    let mut next = before.clone();
    next.post_developer_attention(agent, &format!("[waiting-on-you] {}", "界".repeat(130)), 1)
        .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + 3000),
        Some(format!(
            "libtv-app master is waiting on you: {}",
            "界".repeat(130)
        ))
    );
}

#[test]
fn developer_notification_non_attention_author_can_use_waiting_tag() {
    let (mut before, master, _, _) = fixture();
    let ordinary = before
        .create_agent(
            master,
            "plain sender",
            Provider::ClaudeCode,
            "/tmp".into(),
            None,
        )
        .unwrap();
    let mut next = before.clone();
    next.post_to_developer(
        master,
        ordinary,
        format!(
            "{} [waiting-on-you] quoted literally",
            crate::messaging::attention::TAG
        ),
        vec![],
        1,
    )
    .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + 3000).as_deref(),
        Some("plain sender is waiting on you: quoted literally")
    );
}

#[test]
fn developer_notification_matches_tag_and_equivalent_whole_phrases() {
    for marker in [
        "[waiting-on-you]",
        "[waiting_on_you]",
        "[WAITING_ON_YOU]",
        "[WAITING-ON-YOU]",
        "[WaItInG-oN-yOu]",
        "waiting on you",
        "still waiting on you",
        "WAITING ON YOU",
        "StIlL WaItInG On YoU",
    ] {
        let (before, master, _, agent) = fixture();
        let mut next = before.clone();
        next.post_to_developer(
            master,
            agent,
            format!("Summary\n{marker}: Approve access\nPRIVATE"),
            vec![],
            1,
        )
        .unwrap();
        let mut policy = enabled();
        let now = 1000_u64;
        policy.observe(&before, &next, now);
        assert_eq!(
            policy.take(now + 2000).as_deref(),
            Some("libtv-app master is waiting on you: Approve access"),
            "{marker}"
        );
    }
}

#[test]
fn developer_notification_untagged_peer_messages_and_partial_phrases_are_quiet() {
    for text in [
        "Need access",
        "[needs_developer_attention] Need access",
        "awaiting on you: access",
        "waiting on yourself",
        "waiting on you_all",
        "waiting on your reply",
        "界waiting on you",
        "waiting on you界",
    ] {
        let (before, master, _, agent) = fixture();
        let mut next = before.clone();
        next.submit_message_from(
            master,
            crate::messaging::model::Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Agent(agent),
            1,
        )
        .unwrap();
        let mut policy = enabled();
        let now = 1000_u64;
        policy.observe(&before, &next, now);
        assert!(policy.take(now + 3000).is_none(), "{text}");
    }
}

#[test]
fn developer_notification_untagged_direct_messages_join_a_burst() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(master, agent, "Untagged".into(), vec![], 1)
        .unwrap();
    next.post_to_developer(master, agent, "[waiting-on-you] Approve".into(), vec![], 2)
        .unwrap();
    next.post_to_developer(master, agent, "Untagged too".into(), vec![], 3)
        .unwrap();
    let mut policy = enabled();
    let now = 1000_u64;
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + 2000).as_deref(),
        Some("libtv-app master to you: Untagged\nlibtv-app master is waiting on you: Approve\nlibtv-app master to you: Untagged too")
    );
}

#[test]
fn developer_notification_untagged_bus_attention_is_quiet() {
    let (before, _, _, agent) = fixture();
    let mut next = before.clone();
    next.post_developer_attention(agent, "Process exited", 1)
        .unwrap();
    let mut policy = enabled();
    let now = 1000_u64;
    policy.observe(&before, &next, now);
    assert!(policy.take(now + 3000).is_none());
}

#[test]
fn developer_notification_untagged_final_is_quiet() {
    let (mut before, master, _, agent) = fixture();
    before.set_draft_text(master, "status").unwrap();
    before.set_draft_recipients(master, [agent]).unwrap();
    let request = before.submit_draft(master, 1).unwrap()[0];
    let mut value = serde_json::to_value(&before).unwrap();
    let item = &mut value["requests"][request.0.to_string()];
    item["phase"] = serde_json::json!("completed");
    item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"Complete", "received_at_ms":2, "provider_session_id":null, "provider_turn_id":null});
    item["completed_at_ms"] = serde_json::json!(2);
    let next: BusState = serde_json::from_value(value).unwrap();
    let mut policy = enabled();
    let now = 1000_u64;
    policy.observe(&before, &next, now);
    assert!(policy.take(now + 3000).is_none());
}

#[test]
fn developer_notification_markdown_bold_phrase_heading_previews_the_next_bullet() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(
        master,
        agent,
        "Gate passed.\n\n**Waiting on you:**\n- Approve claude-dev's push of feat/gallery".into(),
        vec![],
        1,
    )
    .unwrap();
    let mut policy = enabled();
    let now = 1000_u64;
    policy.observe(&before, &next, now);
    let text = policy.take(now + 2000).unwrap();
    assert!(
        text.contains("Approve claude-dev's push of feat/gallery"),
        "{text}"
    );
}

#[test]
fn developer_notification_markdown_markers_preserve_the_action() {
    for (message, expected) in [
        ("**[waiting-on-you]**\n- Approve", "- Approve"),
        ("`[WAITING-ON-YOU]`: Approve", "Approve"),
        ("**waiting on you**: Approve", "Approve"),
        ("__[waiting-on-you]__\n- Approve", "- Approve"),
        ("[waiting-on-you] _identifier", "_identifier"),
        ("[waiting-on-you] * Approve", "* Approve"),
    ] {
        assert_eq!(waiting_action(message), Some(expected), "{message}");
    }
}

#[test]
fn notify_you_worker_posts_and_tagged_peer_messages_in_every_room() {
    let (before, master, work, orchestrator) = fixture();
    let mut before = before;
    let worker = before
        .create_agent(work, "worker", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    for room in [master, work] {
        let mut next = before.clone();
        next.post_to_developer(room, worker, "Decide release timing".into(), vec![], 1)
            .unwrap();
        let mut policy = enabled();
        policy.observe(&before, &next, 1000);
        assert_eq!(
            policy.take(3000).as_deref(),
            Some("worker to you: Decide release timing")
        );
    }
    let mut next = before.clone();
    next.submit_message_from(
        work,
        crate::messaging::model::Draft {
            text: "[waiting-on-you] Approve access".into(),
            files: vec![],
            recipient_ids: [orchestrator].into(),
        },
        Author::Agent(worker),
        1,
    )
    .unwrap();
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    assert_eq!(
        policy.take(3000).as_deref(),
        Some("worker is waiting on you: Approve access")
    );
}

#[test]
fn notify_you_tagged_worker_final_to_an_agent_notifies_once() {
    let (mut before, _, work, orchestrator) = fixture();
    let worker = before
        .create_agent(work, "worker", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    let request = before
        .submit_message_from(
            work,
            crate::messaging::model::Draft {
                text: "Inspect".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Agent(orchestrator),
            1,
        )
        .unwrap()[0];
    let mut value = serde_json::to_value(&before).unwrap();
    let item = &mut value["requests"][request.0.to_string()];
    item["phase"] = serde_json::json!("completed");
    item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"[waiting-on-you] Decide release", "received_at_ms":2, "provider_session_id":null, "provider_turn_id":null});
    item["completed_at_ms"] = serde_json::json!(2);
    let next: BusState = serde_json::from_value(value).unwrap();
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    assert_eq!(
        policy.take(3000).as_deref(),
        Some("worker is waiting on you: Decide release")
    );
    policy.observe(&next, &next, 4000);
    assert!(policy.take(6000).is_none());
}

#[test]
fn notify_you_developer_own_tagged_prompt_never_notifies() {
    let (before, _, work, agent) = fixture();
    let mut next = before.clone();
    next.submit_message_from(
        work,
        crate::messaging::model::Draft {
            text: "[waiting-on-you] Developer text".into(),
            files: vec![],
            recipient_ids: [agent].into(),
        },
        Author::Developer,
        1,
    )
    .unwrap();
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    assert!(policy.take(4000).is_none());
}

#[test]
fn notify_you_forwarded_worker_reply_and_ack_do_not_repeat_the_alert() {
    let (mut before, _, work, orchestrator) = fixture();
    let worker = before
        .create_agent(work, "worker", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    let request = before
        .submit_message_from(
            work,
            crate::messaging::model::Draft {
                text: "Inspect".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Agent(orchestrator),
            1,
        )
        .unwrap()[0];
    let mut value = serde_json::to_value(&before).unwrap();
    let item = &mut value["requests"][request.0.to_string()];
    item["phase"] = serde_json::json!("completed");
    item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"[waiting-on-you] Decide release", "received_at_ms":2, "provider_session_id":null, "provider_turn_id":null});
    item["completed_at_ms"] = serde_json::json!(2);
    let mut next: BusState = serde_json::from_value(value).unwrap();
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    assert!(policy.take(3000).is_some());
    let completed = next.clone();
    assert!(next
        .route_worker_replies(5000, &std::collections::BTreeSet::new())
        .unwrap());
    policy.observe(&completed, &next, 5000);
    assert!(policy.take(8000).is_none());
    let forwarded = next.clone();
    next.see_worker_reply(request, orchestrator, 9000).unwrap();
    policy.observe(&forwarded, &next, 9000);
    assert!(policy.take(12000).is_none());
}
