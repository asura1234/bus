use super::*;
use crate::messaging::model::{AgentId, Author, Provider, RoomId};
use std::time::Duration;

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

fn enabled() -> DeveloperNotifications {
    let mut policy = DeveloperNotifications::default();
    policy.configure(true, false, None);
    policy
}

#[test]
fn developer_notification_to_human_has_only_sender_and_first_line() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(
        master,
        agent,
        "Need Figma access\nPRIVATE second line".into(),
        vec![],
        1,
    )
    .unwrap();
    let now = Instant::now();
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    assert!(policy.take(now + Duration::from_secs(1)).is_none());
    assert_eq!(
        policy.take(now + Duration::from_secs(2)).as_deref(),
        Some("libtv-app master: Need Figma access")
    );
    assert!(policy.take(now + Duration::from_secs(4)).is_none());
    policy.observe(&next, &next, now + Duration::from_secs(5));
    assert!(policy.take(now + Duration::from_secs(8)).is_none());
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
    item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"Still waiting on you", "received_at_ms":2, "provider_session_id":null, "provider_turn_id":null});
    item["completed_at_ms"] = serde_json::json!(2);
    let next: BusState = serde_json::from_value(value).unwrap();
    let mut policy = enabled();
    let now = Instant::now();
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + Duration::from_secs(2)).as_deref(),
        Some("libtv-app master: Still waiting on you")
    );
    policy.observe(&next, &next, now + Duration::from_secs(3));
    assert!(policy.take(now + Duration::from_secs(6)).is_none());
}

#[test]
fn developer_notification_viewing_master_suppresses_unless_known_unfocused() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(master, agent, "hello".into(), vec![], 1)
        .unwrap();
    let now = Instant::now();
    for focus in [None, Some(true), Some(false)] {
        let mut policy = enabled();
        policy.configure(true, true, focus);
        policy.observe(&before, &next, now);
        assert_eq!(
            policy.take(now + Duration::from_secs(3)).is_some(),
            focus == Some(false)
        );
    }
}

#[test]
fn developer_notification_coalesces_messages_without_extending_window() {
    let (before, master, _, agent) = fixture();
    let mut first = before.clone();
    first
        .post_to_developer(master, agent, "first".into(), vec![], 1)
        .unwrap();
    let mut second = first.clone();
    second
        .post_to_developer(master, agent, "second".into(), vec![], 2)
        .unwrap();
    let now = Instant::now();
    let mut policy = enabled();
    policy.observe(&before, &first, now);
    policy.observe(&first, &second, now + Duration::from_millis(1900));
    assert_eq!(
        policy.take(now + Duration::from_secs(2)).as_deref(),
        Some("libtv-app master and 1 more")
    );
    assert!(policy.take(now + Duration::from_secs(5)).is_none());
}

#[test]
fn developer_notification_toggle_and_view_change_discard_pending_without_replay() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(master, agent, "hello".into(), vec![], 1)
        .unwrap();
    let now = Instant::now();
    for (on, visible, focus) in [
        (false, false, None),
        (true, true, None),
        (true, true, Some(true)),
    ] {
        let mut policy = enabled();
        policy.observe(&before, &next, now);
        policy.configure(on, visible, focus);
        policy.configure(true, false, None);
        assert!(policy.take(now + Duration::from_secs(3)).is_none());
        policy.observe(&next, &next, now + Duration::from_secs(4));
        assert!(policy.take(now + Duration::from_secs(7)).is_none());
    }
}

#[test]
fn developer_notification_excludes_work_rooms_and_agent_directed_prompts() {
    let (before, master, work, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(work, agent, "work room".into(), vec![], 1)
        .unwrap();
    next.submit_message_from(
        master,
        crate::messaging::model::Draft {
            text: "agent directed".into(),
            files: vec![],
            recipient_ids: [agent].into(),
        },
        Author::Agent(agent),
        2,
    )
    .unwrap();
    let now = Instant::now();
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    assert!(policy.take(now + Duration::from_secs(3)).is_none());
}

#[test]
fn developer_notification_preview_is_bounded_unicode_and_control_free() {
    let (before, master, _, agent) = fixture();
    let mut next = before.clone();
    next.post_to_developer(
        master,
        agent,
        format!("{}\u{1b}\u{7}\nPRIVATE", "界".repeat(130)),
        vec![],
        1,
    )
    .unwrap();
    let now = Instant::now();
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    let text = policy.take(now + Duration::from_secs(2)).unwrap();
    assert_eq!(text.split_once(": ").unwrap().1.chars().count(), 101);
    assert!(text.ends_with('…'));
    assert!(!text.chars().any(char::is_control));
    assert!(!text.contains("PRIVATE"));
}

#[test]
fn developer_notification_carriage_return_ends_the_preview() {
    assert_eq!(plain_preview("first\rPRIVATE", 100), "first");
}

#[test]
fn developer_notification_archive_moves_do_not_replay_notices() {
    let (mut before, master, _, agent) = fixture();
    before
        .post_to_developer(master, agent, "old".into(), vec![], 1)
        .unwrap();
    let mut archived = before.clone();
    assert!(archived.archive(20 * 60 * 1000));
    let now = Instant::now();
    let mut policy = enabled();
    policy.observe(&before, &archived, now);
    assert!(policy.take(now + Duration::from_secs(3)).is_none());
}

fn ui_fixture() -> (BusUi, RoomId, RoomId, AgentId) {
    let (state, master, work, agent) = fixture();
    let mut ui = BusUi::new(std::sync::Arc::new(
        crate::messaging::coordinator::BusSnapshot {
            state,
            revision: 0,
            last_command_id: 0,
            error: None,
        },
    ));
    ui.open_room(work);
    ui.locals.get_mut(&work).unwrap().text.insert("KEEP DRAFT");
    (ui, master, work, agent)
}

#[test]
fn developer_notification_ui_preserves_draft_and_unread_then_suppresses_master() {
    let (mut ui, master, work, agent) = ui_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_to_developer(master, agent, "Need access".into(), vec![], 1)
        .unwrap();
    snapshot.revision += 1;
    let unread = snapshot.state.room(master).unwrap().unread_count;
    ui.receive_snapshot(std::sync::Arc::new(snapshot));
    assert_eq!(
        ui.take_developer_notification(Instant::now() + Duration::from_secs(3))
            .as_deref(),
        Some("libtv-app master: Need access")
    );
    assert_eq!(ui.locals[&work].text.text, "KEEP DRAFT");
    assert_eq!(ui.snapshot.state.room(master).unwrap().unread_count, unread);
    ui.open_room(master);
    ui.outer_focused = Some(true);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_to_developer(master, agent, "Already visible".into(), vec![], 2)
        .unwrap();
    ui.receive_snapshot(std::sync::Arc::new(snapshot));
    assert!(ui
        .take_developer_notification(Instant::now() + Duration::from_secs(3))
        .is_none());
}

#[test]
fn developer_notification_opening_master_cancels_even_if_leaving_before_tick() {
    let (mut ui, master, work, agent) = ui_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_to_developer(master, agent, "Read now".into(), vec![], 1)
        .unwrap();
    ui.receive_snapshot(std::sync::Arc::new(snapshot));
    ui.open_room(master);
    ui.open_room(work);
    assert!(ui
        .take_developer_notification(Instant::now() + Duration::from_secs(3))
        .is_none());
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
        item["pending_final"] = serde_json::json!({"callback_id":"done", "text":"one answer", "received_at_ms":3, "provider_session_id":null, "provider_turn_id":null});
        item["completed_at_ms"] = serde_json::json!(3);
    }
    let next: BusState = serde_json::from_value(value).unwrap();
    let mut policy = enabled();
    let now = Instant::now();
    policy.observe(&before, &next, now);
    assert_eq!(
        policy.take(now + Duration::from_secs(3)).as_deref(),
        Some("libtv-app master: one answer")
    );
}

#[test]
fn developer_notification_typed_bus_attention_uses_identified_sender() {
    let (before, _, _, agent) = fixture();
    let mut next = before.clone();
    next.post_developer_attention(agent, "Process exited", 1)
        .unwrap();
    let now = Instant::now();
    let mut policy = enabled();
    policy.observe(&before, &next, now);
    let text = policy.take(now + Duration::from_secs(3)).unwrap();
    assert!(text.starts_with("libtv-app master: "));
    assert!(text.contains("Process exited"));
}
