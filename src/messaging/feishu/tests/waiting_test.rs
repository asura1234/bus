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

/// The TUI alert route: every developer-addressed message, tagged or not.
fn tui(before: &BusState, next: &BusState) -> Vec<String> {
    new_developer_notifications(before, next, false)
}

#[test]
fn quoted_or_empty_waiting_tags_do_not_page_from_peer_briefs() {
    for text in [
        "- It goes only to that room's bound orchestrator. Never to the developer, MASTER or Feishu. No `[waiting-on-you]`.",
        "A mid-sentence [waiting-on-you] Ask",
        "`[waiting_on_you] Ask`",
        "```\n[waiting-on-you] Ask\n```",
        "[waiting-on-you]",
        "**[waiting-on-you]**: .\n\n---",
    ] {
        let (before, master, _, agent) = fixture();
        let mut next = before.clone();
        next.submit_message_from(master, crate::messaging::model::Draft {
            text: text.into(), files: vec![], recipient_ids: [agent].into(),
        }, Author::Agent(agent), 1).unwrap();
        let mut policy = enabled();
        policy.observe(&before, &next, 1000);
        assert!(policy.take(3000).is_none(), "{text}");
        assert!(tui(&before, &next).is_empty(), "{text}");
    }
}

#[test]
fn waiting_choices_and_next_line_reach_both_alert_routes() {
    for (text, body) in [
        ("[waiting-on-you] .\n\nUnlock the phone", "Unlock the phone"),
        (
            "[waiting-on-you] Pick one:\n- **B (recommended):** Keep the API.\n- **A:** Change it.",
            "Pick one:\n- **B (recommended):** Keep the API.\n- **A:** Change it.",
        ),
    ] {
        let (before, master, _, agent) = fixture();
        let mut next = before.clone();
        next.post_to_developer(master, agent, text.into(), vec![], 1)
            .unwrap();
        let expected = format!("libtv-app master is waiting on you: {body}");
        let mut policy = enabled();
        policy.observe(&before, &next, 1000);
        assert_eq!(policy.take(3000).as_deref(), Some(expected.as_str()));
        assert_eq!(tui(&before, &next), [expected]);
        // The TUI history retains the complete original notice, independently of the alert body.
        assert_eq!(next.notices(master).last().unwrap().text, text);
    }
}

#[cfg(unix)]
#[test]
fn waiting_multiline_body_is_one_literal_feishu_text_argument() {
    use std::os::unix::fs::PermissionsExt;
    let root = crate::utils::test_temp::unique_temp_path("waiting-multiline");
    std::fs::create_dir_all(&root).unwrap();
    let script = root.join("lark-cli");
    std::fs::write(&script, "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '--text' ]; then shift; printf '%s' \"$1\" > \"${0%/*}/text\"; break; fi\n  shift\ndone\nprintf '{\"ok\":true}'\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(
        master,
        agent,
        "[waiting-on-you] Pick one:\n- **B:** Keep `$HOME`.\n- **A:** Change it.".into(),
        vec![],
        1,
    )
    .unwrap();
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    let body = policy.take(3000).unwrap();
    let client = crate::messaging::feishu::Client::default();
    client
        .send(
            "ou_test",
            script.to_str(),
            &body,
            "test",
            &crate::messaging::feishu::Cancellation::default(),
        )
        .unwrap();
    assert_eq!(std::fs::read_to_string(root.join("text")).unwrap(), body);
    assert!(body.contains("\n- **B:** Keep `$HOME`.\n- **A:** Change it."));
    std::fs::remove_dir_all(root).unwrap();
}

fn enabled() -> WaitingNotifications {
    let mut policy = WaitingNotifications::default();
    policy.configure(true);
    policy
}

#[test]
fn developer_notification_to_human_keeps_following_action_lines() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(
        master,
        agent,
        "Summary\n[waiting-on-you]\n- Need Figma access\n- Grant project access too".into(),
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
        Some("libtv-app master is waiting on you: - Need Figma access\n- Grant project access too")
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
    assert_eq!(
        tui(&before, &next),
        ["libtv-app master is waiting on you: Approve export"]
    );
    assert!(tui(&next, &next).is_empty());
    // The legacy phrase is not a Feishu page; only the tag is.
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    assert!(policy.take(3000).is_none());
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
        .post_to_developer(master, agent, "[waiting-on-you] second".into(), vec![], 2)
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
        format!(
            "[waiting-on-you] {}\u{1b}\u{7}\nNext\tstep\u{7}",
            "界".repeat(130)
        ),
        vec![],
        1,
    )
    .unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    let text = policy.take(now + 2000).unwrap();
    assert_eq!(
        text.split_once(": ").unwrap().1,
        format!("{}\nNext step", "界".repeat(130))
    );
    assert!(!text.chars().any(|ch| ch.is_control() && ch != '\n'));
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
fn developer_notification_tagged_final_keeps_following_action_lines() {
    let (mut before, master, _, agent) = fixture();
    before.set_draft_text(master, "status").unwrap();
    before.set_draft_recipients(master, [agent]).unwrap();
    let request = before.submit_draft(master, 1).unwrap()[0];
    let mut value = serde_json::to_value(&before).unwrap();
    let item = &mut value["requests"][request.0.to_string()];
    item["phase"] = serde_json::json!("completed");
    item["pending_final"] = serde_json::json!({"callback_id":"done", "text":format!("{}\n[waiting-on-you]\nNeed Figma access\nGrant project access too", crate::messaging::attention::TAG), "received_at_ms":2, "provider_session_id":null, "provider_turn_id":null});
    item["completed_at_ms"] = serde_json::json!(2);
    let next: BusState = serde_json::from_value(value).unwrap();
    let now = 1000_u64;
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + 3000).as_deref(),
        Some("libtv-app master is waiting on you: Need Figma access\nGrant project access too")
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
            format!("Summary\n{marker}: Approve access\nGrant project access too"),
            vec![],
            1,
        )
        .unwrap();
        assert_eq!(
            tui(&before, &next),
            ["libtv-app master is waiting on you: Approve access\nGrant project access too"],
            "{marker}"
        );
        let mut policy = enabled();
        policy.observe(&before, &next, 1000);
        let paged = policy.take(3000);
        if marker.starts_with('[') {
            assert_eq!(
                paged.as_deref(),
                Some(
                    "libtv-app master is waiting on you: Approve access\nGrant project access too"
                ),
                "{marker}"
            );
        } else {
            assert!(paged.is_none(), "phrase {marker} must not page Feishu");
        }
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
    assert_eq!(
        tui(&before, &next),
        [
            "libtv-app master to you: Untagged",
            "libtv-app master is waiting on you: Approve",
            "libtv-app master to you: Untagged too"
        ]
    );
    // Feishu carries only the tagged hard blocker.
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    assert_eq!(
        policy.take(3000).as_deref(),
        Some("libtv-app master is waiting on you: Approve")
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
    let text = tui(&before, &next).join("\n");
    assert!(
        text.contains("Approve claude-dev's push of feat/gallery"),
        "{text}"
    );
}

#[test]
fn developer_notification_markdown_markers_preserve_the_action() {
    for (message, expected) in [
        ("**[waiting-on-you]**\n- Approve", "- Approve"),
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
        assert_eq!(
            tui(&before, &next),
            ["worker to you: Decide release timing"]
        );
        let mut policy = enabled();
        policy.observe(&before, &next, 1000);
        assert!(
            policy.take(3000).is_none(),
            "untagged posts stay in the TUI"
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

#[test]
fn untagged_waiting_phrase_between_agents_does_not_alert_the_developer() {
    let (mut before, _, work, orchestrator) = fixture();
    let worker = before
        .create_agent(work, "worker", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    let mut next = before.clone();
    // "you" here is the orchestrator the worker addresses, not the developer.
    next.submit_message_from(
        work,
        crate::messaging::model::Draft {
            text: "PR is pushed; still waiting on you for the next task".into(),
            files: vec![],
            recipient_ids: [orchestrator].into(),
        },
        Author::Agent(worker),
        1,
    )
    .unwrap();
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    assert_eq!(policy.take(3000), None);
}

#[test]
fn untagged_waiting_phrase_in_a_worker_final_to_an_agent_does_not_alert_the_developer() {
    let (mut before, _, work, orchestrator) = fixture();
    let worker = before
        .create_agent(work, "worker", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    let request = before
        .submit_message_from(
            work,
            crate::messaging::model::Draft {
                text: "Rebase and report".into(),
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
    item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"BLOCKED: waiting on you to pick the base branch", "received_at_ms":2, "provider_session_id":null, "provider_turn_id":null});
    item["completed_at_ms"] = serde_json::json!(2);
    let next: BusState = serde_json::from_value(value).unwrap();
    let mut policy = enabled();
    policy.observe(&before, &next, 1000);
    assert_eq!(policy.take(3000), None);
}

#[test]
fn explicit_you_mention_in_peer_traffic_alerts_but_partial_tokens_do_not() {
    let (mut before, _, work, orchestrator) = fixture();
    let worker = before
        .create_agent(work, "worker", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    for (text, alerts) in [
        ("@you Please choose the rollout", true),
        ("(@YOU) choose", true),
        ("Ask @yourself", false),
        ("dev@you.example", false),
        ("@you_all", false),
    ] {
        let mut next = before.clone();
        next.submit_message_from(
            work,
            crate::messaging::model::Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [orchestrator].into(),
            },
            Author::Agent(worker),
            1,
        )
        .unwrap();
        assert_eq!(!tui(&before, &next).is_empty(), alerts, "{text}");
        let mut policy = enabled();
        policy.observe(&before, &next, 1000);
        assert!(
            policy.take(3000).is_none(),
            "@you never pages Feishu: {text}"
        );
    }
}

#[test]
fn coalesced_developer_and_peer_requests_keep_developer_phrase_context() {
    let (mut before, _, work, orchestrator) = fixture();
    let worker = before
        .create_agent(work, "worker", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    let first = before
        .submit_message_from(
            work,
            crate::messaging::model::Draft {
                text: "Inspect".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Developer,
            1,
        )
        .unwrap()[0];
    let second = before
        .submit_message_from(
            work,
            crate::messaging::model::Draft {
                text: "Also inspect the logs".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Agent(orchestrator),
            2,
        )
        .unwrap()[0];
    assert!(before.coalesce_queue(first).is_some());
    let mut value = serde_json::to_value(&before).unwrap();
    for request in [first, second] {
        let item = &mut value["requests"][request.0.to_string()];
        item["phase"] = serde_json::json!("completed");
        item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"Still waiting on you: Approve access", "received_at_ms":3, "provider_session_id":null, "provider_turn_id":null});
        item["completed_at_ms"] = serde_json::json!(3);
    }
    let next: BusState = serde_json::from_value(value).unwrap();
    assert_eq!(
        tui(&before, &next),
        ["worker is waiting on you: Approve access"]
    );
}

#[test]
fn repeated_tagged_ask_from_the_same_sender_pages_once_per_half_hour() {
    let (before, master, _, agent) = fixture();
    let mut first = before.clone();
    first
        .post_to_developer(
            master,
            agent,
            "[waiting-on-you] Restart Bus".into(),
            vec![],
            1,
        )
        .unwrap();
    let mut second = first.clone();
    second
        .post_to_developer(
            master,
            agent,
            "[waiting-on-you] Restart Bus".into(),
            vec![],
            2,
        )
        .unwrap();
    let mut policy = enabled();
    policy.observe(&before, &first, 1000);
    assert!(policy.take(3000).is_some());
    policy.observe(&first, &second, 60_000);
    assert!(policy.take(70_000).is_none(), "same ask within 30 minutes");
    let mut third = second.clone();
    third
        .post_to_developer(
            master,
            agent,
            "[waiting-on-you] Restart Bus".into(),
            vec![],
            3,
        )
        .unwrap();
    policy.observe(&second, &third, 31 * 60_000 + 1000);
    assert!(policy.take(31 * 60_000 + 4000).is_some());
}
