use std::path::PathBuf;

use super::*;

#[path = "send_recovery_test.rs"]
mod send_recovery_tests;

#[path = "shell_hold_test.rs"]
mod shell_hold_tests;

#[path = "unconfirmed_submission_test.rs"]
mod unconfirmed_submission_tests;

fn master_agent(state: &mut BusState, name: &str) -> AgentId {
    let master = state.ensure_master_room();
    state
        .create_agent(master, name, Provider::ClaudeCode, "/repo".into(), None)
        .expect("master agent")
}

#[test]
fn developer_attention_mechanical_episode_is_atomic_durable_and_rearms_only_on_recovery() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let agent = master_agent(&mut state, "orch");
    state.bind_orchestrator(agent, work).unwrap();
    let key = "exit:1:launch".to_string();
    state
        .attention_episode(key.clone(), Some(true), agent, "Process exited", 100)
        .unwrap();
    let notices: Vec<_> = state.notices(master).collect();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].author, Author::Bus);
    assert_eq!(notices[0].attention_origin.as_ref().unwrap().room_id, work);
    assert!(notices[0]
        .text
        .starts_with(crate::messaging::attention::TAG));
    let mut state: BusState = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
    for active in [Some(true), None, Some(true)] {
        state
            .attention_episode(key.clone(), active, agent, "Process exited", 101)
            .unwrap();
    }
    assert_eq!(state.notices(master).count(), 1);
    state
        .attention_episode(key.clone(), Some(false), agent, "", 102)
        .unwrap();
    state
        .attention_episode(key, Some(true), agent, "Process exited", 103)
        .unwrap();
    assert_eq!(state.notices(master).count(), 2);
}

#[test]
fn feishu_prunes_delivery_guards_after_their_history_is_removed() {
    let mut state = BusState::default();
    state.attention_delivery.handled.insert("notice:999".into());
    state
        .attention_delivery
        .interrupted
        .insert("notice:999".into());
    state
        .attention_episodes
        .insert("exit:999:old-launch".into());
    state.attention_episodes.insert("stall:999".into());
    assert!(state.prune_attention());
    assert!(state.attention_delivery.handled.is_empty());
    assert!(state.attention_delivery.interrupted.is_empty());
    assert!(state.attention_episodes.is_empty());
    assert!(!state.prune_attention());
}

#[test]
fn developer_attention_stall_uses_existing_deadline_and_ignores_working_or_blocked() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    state.confirm_hook_setup(agent).unwrap();
    let request = submit_text(&mut state, room, agent, "queued");
    let master = state.ensure_master_room();
    state
        .notify_stalled_attention(10 + QUEUED_STALL_MS - 1)
        .unwrap();
    assert_eq!(state.notices(master).count(), 0);
    state
        .notify_stalled_attention(10 + QUEUED_STALL_MS)
        .unwrap();
    state
        .notify_stalled_attention(10 + QUEUED_STALL_MS + 1)
        .unwrap();
    assert_eq!(state.notices(master).count(), 1);
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Queued);
    let mut recovered = state.clone();
    let recovery = 10 + QUEUED_STALL_MS + 10;
    recovered
        .observe_status(agent, RuntimeStatus::Working, recovery)
        .unwrap();
    recovered.notify_stalled_attention(recovery + 1).unwrap();
    recovered
        .observe_status(agent, RuntimeStatus::Idle, recovery + 2)
        .unwrap();
    recovered
        .notify_stalled_attention(recovery + 2 + QUEUED_STALL_MS)
        .unwrap();
    assert_eq!(
        recovered.notices(master).count(),
        1,
        "working then idle must not re-arm the same stalled request"
    );
    let serialized = serde_json::to_vec(&recovered).unwrap();
    let mut recovered: BusState = serde_json::from_slice(&serialized).unwrap();
    recovered.prune_attention();
    recovered.notify_stalled_attention(u64::MAX).unwrap();
    assert_eq!(recovered.notices(master).count(), 1);
    let another = submit_text(&mut recovered, room, agent, "another request");
    assert_ne!(request, another);
    recovered.notify_stalled_attention(u64::MAX).unwrap();
    assert_eq!(
        recovered.notices(master).count(),
        2,
        "a distinct stalled request still alerts"
    );
    for status in [RuntimeStatus::Working, RuntimeStatus::Blocked] {
        let mut state = state.clone();
        state.attention_episodes.clear();
        state.rooms.get_mut(&master).unwrap().notices.clear();
        state.observe_status(agent, status, 20).unwrap();
        state.notify_stalled_attention(u64::MAX).unwrap();
        assert_eq!(state.notices(master).count(), 0);
    }
}

#[test]
fn every_bus_notice_survives_a_roundtrip_with_or_without_an_attention_origin() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let agent = master_agent(&mut state, "orch");
    state.bind_orchestrator(agent, work).unwrap();
    for text in [
        "[needs_developer_attention] Process exited",
        "impl reached 5 compactions; get a handover note and replace it.",
    ] {
        state
            .post_to_developer(master, agent, text.into(), vec![], 10)
            .unwrap();
    }
    let mut encoded = serde_json::to_value(&state).unwrap();
    let notices = encoded["rooms"][master.0.to_string()]["notices"]
        .as_array_mut()
        .unwrap();
    for notice in notices.iter_mut() {
        notice["author"] = serde_json::to_value(Author::Bus).unwrap();
    }
    notices[0]["attention_origin"] = serde_json::json!({
        "room_id": work.0, "room_name": "work", "agent_id": agent.0, "agent_name": "orch"
    });
    let reopened: BusState = serde_json::from_value(encoded).unwrap();
    let notices: Vec<_> = reopened.notices(master).collect();
    assert_eq!(notices.len(), 2, "every Bus notice must remain durable");
    assert!(notices[0].text.contains("Process exited"));
    assert!(notices[1].text.contains("reached 5 compactions"));
    assert!(notices.iter().all(|notice| notice.author == Author::Bus));
    let saved = serde_json::to_value(&reopened).unwrap();
    assert_eq!(
        saved["rooms"][master.0.to_string()]["notices"][0]["attention_origin"]["agent_name"],
        "orch"
    );
}

#[test]
fn developer_attention_episode_latches_survive_model_roundtrip() {
    let mut encoded = serde_json::to_value(BusState::default()).unwrap();
    encoded["attention_episodes"] = serde_json::json!(["exit:agent-1:launch-1"]);
    let reopened: BusState = serde_json::from_value(encoded).unwrap();
    let saved = serde_json::to_value(reopened).unwrap();
    assert_eq!(
        saved["attention_episodes"],
        serde_json::json!(["exit:agent-1:launch-1"])
    );
}

#[test]
fn feishu_delivery_bookkeeping_survives_model_roundtrip() {
    let mut encoded = serde_json::to_value(BusState::default()).unwrap();
    encoded["attention_delivery"] = serde_json::json!({
        "initialized": true, "handled": ["notice:1", "notice:2"], "interrupted": ["notice:2"],
        "recent_attempts": [{"source":"notice:2", "signature":"abc", "at_ms":10}]
    });
    let reopened: BusState = serde_json::from_value(encoded).unwrap();
    let saved = serde_json::to_value(reopened).unwrap();
    assert_eq!(saved["attention_delivery"]["initialized"], true);
    assert_eq!(
        saved["attention_delivery"]["handled"],
        serde_json::json!(["notice:1", "notice:2"])
    );
    assert_eq!(
        saved["attention_delivery"]["interrupted"],
        serde_json::json!(["notice:2"])
    );
}

#[test]
fn feishu_delivery_and_episode_changes_require_a_durable_save() {
    let before = BusState::default();
    let mut encoded = serde_json::to_value(&before).unwrap();
    encoded["attention_delivery"] = serde_json::json!({"initialized":true,"handled":["notice:1"]});
    let delivery: BusState = serde_json::from_value(encoded.clone()).unwrap();
    assert!(delivery.durable_poll_change_from(&before));
    encoded["attention_episodes"] = serde_json::json!(["exit:1"]);
    let episode: BusState = serde_json::from_value(encoded).unwrap();
    assert!(episode.durable_poll_change_from(&delivery));
}

fn serialized_agent_color(state: &BusState, id: AgentId) -> [u8; 3] {
    let document = serde_json::to_value(state).expect("serialize state");
    serde_json::from_value(document["agents"][id.0.to_string()]["color"].clone())
        .expect("each agent has a persisted RGB color")
}

fn serialized_accessible_color(state: &BusState, id: AgentId) -> [u8; 3] {
    let document = serde_json::to_value(state).expect("serialize state");
    serde_json::from_value(document["agents"][id.0.to_string()]["accessible_color"].clone())
        .expect("each agent has a persisted color blind mode RGB color")
}

fn state_with_room_and_agents() -> (BusState, RoomId, AgentId, AgentId) {
    let mut state = BusState::new();
    let room = state.create_room("launch").expect("room");
    let codex = state
        .create_agent(
            room,
            "builder",
            Provider::Codex,
            PathBuf::from("/repo"),
            Some("main".into()),
        )
        .expect("agent");
    let claude = state
        .create_agent(
            room,
            "reviewer",
            Provider::ClaudeCode,
            PathBuf::from("/repo"),
            Some("topic".into()),
        )
        .expect("agent");
    state
        .set_agent_runtime_identity(
            codex,
            AgentRuntimeIdentity {
                launch_id: Some("launch-codex".into()),
                terminal_id: Some("terminal-1".into()),
                pane_id: Some("pane-1".into()),
                session_id: Some("bus-1".into()),
            },
        )
        .expect("identity");
    state
        .set_agent_runtime_identity(
            claude,
            AgentRuntimeIdentity {
                launch_id: Some("launch-claude".into()),
                terminal_id: None,
                pane_id: None,
                session_id: None,
            },
        )
        .expect("identity");
    (state, room, codex, claude)
}

#[test]
fn older_saved_state_defaults_deletion_flags_to_false() {
    let (state, room, agent, _) = state_with_room_and_agents();
    let mut saved = serde_json::to_value(&state).unwrap();
    for collection in ["rooms", "agents"] {
        for value in saved[collection].as_object_mut().unwrap().values_mut() {
            value.as_object_mut().unwrap().remove("deletion_pending");
        }
    }
    let recovered: BusState = serde_json::from_value(saved).unwrap();
    assert!(!recovered.room(room).unwrap().deletion_pending);
    assert!(!recovered.agent(agent).unwrap().deletion_pending);
    assert_eq!(recovered, state);
}

fn submit_text(state: &mut BusState, room: RoomId, agent: AgentId, text: &str) -> RequestId {
    state.set_draft_text(room, text).expect("draft text");
    state
        .set_draft_recipients(room, [agent])
        .expect("recipients");
    state.submit_draft(room, 10).expect("submit")[0]
}

/// Types `request` and records the submission, with no provider turn start yet.
fn submit_request(state: &mut BusState, request: RequestId, launch_id: &str, boundary: u64) {
    state
        .begin_submission(request, launch_id, boundary)
        .expect("begin submission");
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .expect("record submission");
}

fn start_request(state: &mut BusState, request: RequestId, launch_id: &str, boundary: u64) {
    submit_request(state, request, launch_id, boundary);
    let request_state = state.request(request).expect("request");
    let agent_id = request_state.agent_id;
    let prompt_payload = request_state.prompt.rendered_payload();
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: format!("trusted-start-{}", request.0),
            sequence: boundary + 1,
            occurred_at_ms: boundary + 1,
            agent_id,
            launch_id: launch_id.into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some(prompt_payload),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
}

#[test]
fn claude_image_placeholders_only_stand_for_lone_image_path_lines() {
    let typed = "Look\n\n\"/tmp/a.png\"";
    assert!(payload_matches("[Image #2]Look", typed));
    assert!(payload_matches("Look\n\n\"/tmp/a.png\"", typed));
    // Two paths on one line, or a non-image file, stay as typed.
    assert!(!payload_matches(
        "[Image #1]Look",
        "Look\n\"/tmp/a.png\" \"/tmp/b.png\""
    ));
    assert!(!payload_matches("[Image #1]Look", "Look\n\"/tmp/a.diff\""));
    assert!(!payload_matches("[Image #1]", "Look\n\"/tmp/a.png\""));
    assert!(!payload_matches("[Image #x]Look", typed));
}

/// A codex agent idle since `at`, with one message `text` queued.
fn idle_with_queued(at: u64, text: &str) -> (BusState, AgentId, RequestId) {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Idle, at)
        .unwrap();
    let request = submit_text(&mut state, room, agent, text);
    (state, agent, request)
}

fn stall_at(state: &BusState, request: RequestId, now_ms: u64) -> Option<String> {
    state.stall_reason(state.request(request).unwrap(), now_ms)
}

#[test]
fn native_refusals_map_to_stall_reasons() {
    // The exact texts `src/server/api/agents/prompt.rs` sends.
    for (message, reason) in [
        (
            "Claude input box is not empty; prompt was not sent",
            "input_box_not_empty",
        ),
        (
            "Codex composer is not empty; prompt was not sent",
            "input_box_not_empty",
        ),
        ("Agent is not ready; prompt was not sent", "agent_not_ready"),
        (
            "agent w1:p1 is not an active named agent",
            "agent_not_ready",
        ),
        ("agent is blocked on a dialog", "agent_blocked"),
        ("something else", "delivery_rejected"),
    ] {
        assert_eq!(rejection_reason(message), reason, "{message}");
    }
}

#[path = "agents_test.rs"]
mod agents;
#[path = "callbacks_test.rs"]
mod callbacks;
#[path = "requests_test.rs"]
mod requests;
#[path = "rooms_test.rs"]
mod rooms;

#[test]
fn claude_image_placeholder_matches_when_text_before_the_image_ends_in_whitespace() {
    for text in ["Look ", "Look\t", "Look \t\r\n"] {
        let prompt = Prompt {
            id: PromptId(1),
            author: Author::Developer,
            text: text.into(),
            files: vec![PathBuf::from("/tmp/a.png")],
            recipient_ids: AgentRecipients::default(),
            submitted_at_ms: 0,
            attention_origin: None,
        };
        // Whether the hook keeps the trailing space or not, the lifted image
        // leaves the typed text line as the whole remaining prompt.
        assert!(prompt.matches_callback_payload("[Image #1]Look "));
        assert!(prompt.matches_callback_payload("[Image #1]Look"));
        assert!(prompt.matches_callback_payload(&prompt.rendered_payload()));
        assert_eq!(prompt.text, text);
    }
}

#[test]
fn claude_image_placeholder_matching_preserves_internal_and_non_ascii_whitespace() {
    let typed = "First \t\n  Last \t\n\"/tmp/a.png\"";
    assert!(payload_matches("[Image #1]First \t\n  Last \t", typed));
    assert!(!payload_matches("[Image #1]First\n  Last", typed));
    assert!(!payload_matches("[Image #1]First \t\nLast", typed));
    assert!(!payload_matches(
        "[Image #1][Image #2]First \t\n  Last",
        typed
    ));

    let typed = "Look\u{a0}\n\"/tmp/a.png\"";
    assert!(payload_matches("[Image #1]Look\u{a0}", typed));
    assert!(!payload_matches("[Image #1]Look", typed));
}

#[test]
fn claude_image_placeholder_callbacks_bind_and_complete_prompt_with_trailing_whitespace() {
    let (mut state, room, _, agent) = state_with_room_and_agents();
    state.attach_file(room, "/tmp/a.png".into()).unwrap();
    let request = submit_text(&mut state, room, agent, "Look \t");
    let queued = submit_text(&mut state, room, agent, "queued next");
    submit_request(&mut state, request, "launch-claude", 10);

    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "image-start".into(),
            sequence: 11,
            occurred_at_ms: 11,
            agent_id: agent,
            launch_id: "launch-claude".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("[Image #1]Look \t".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "image-final",
            12,
            agent,
            "launch-claude",
            "provider-session",
            "turn-1",
            "[Image #1]Look",
            "finished",
        )),
        CallbackDisposition::AcceptedPendingSettlement
    );
    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .unwrap();

    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        state.room(room).unwrap().latest_replies[&agent].text,
        "finished"
    );
    assert_eq!(state.next_queued_request(agent), Some(queued));
    assert_eq!(state.request(request).unwrap().prompt.text, "Look \t");
}

#[test]
fn retired_github_watch_legacy_state_load_preserves_rooms_and_drops_watch_fields() {
    let mut state = BusState::default();
    let room = state.create_room("keep-room").unwrap();
    state
        .set_room_notes(room, "keep these notes".into())
        .unwrap();
    let mut document = serde_json::to_value(&state).unwrap();
    document["github_watch"] = serde_json::json!({"owner/repo":{"scan_started_ms":1}});
    document["github_unavailable"] = serde_json::json!([[room.0, "owner/repo"]]);
    let saved_room = &mut document["rooms"][room.0.to_string()];
    saved_room["github_repos"] = serde_json::json!(["owner/repo"]);
    saved_room["github_discovered"] = serde_json::json!(["owner/repo"]);
    for field in [
        "github_branches",
        "github_member_branches",
        "github_retired_branches",
    ] {
        saved_room[field] = serde_json::json!({"owner/repo":["feature"]});
    }
    let loaded: BusState = serde_json::from_value(document).unwrap();
    assert_eq!(loaded.room(room).unwrap().notes, "keep these notes");
    let written = serde_json::to_value(loaded).unwrap();
    assert!(written.get("github_watch").is_none());
    assert!(written.get("github_unavailable").is_none());
    for field in [
        "github_repos",
        "github_discovered",
        "github_branches",
        "github_member_branches",
        "github_retired_branches",
    ] {
        assert!(
            written["rooms"][room.0.to_string()].get(field).is_none(),
            "retired watch field {field} was written again"
        );
    }
}
