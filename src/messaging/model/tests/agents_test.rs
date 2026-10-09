use super::*;
use std::collections::BTreeSet;

#[test]
fn agent_color_assignment_maximizes_room_separation_and_reserves_you() {
    let mut state = BusState::new();
    let room = state.create_room("colors").unwrap();
    // Independent reference calculation over the readable 17-step sRGB grid,
    // using Euclidean Oklab distance with [102, 255, 102] already occupied.
    for expected in [
        [221, 0, 255],
        [170, 119, 51],
        [255, 204, 255],
        [0, 136, 255],
    ] {
        let id = state
            .create_agent(room, "agent", Provider::Codex, "/repo".into(), None)
            .unwrap();
        assert_eq!(serialized_agent_color(&state, id), expected);
    }
    let another_room = state.create_room("independent room").unwrap();
    let id = state
        .create_agent(
            another_room,
            "reviewer",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    assert_eq!(serialized_agent_color(&state, id), [221, 0, 255]);
}

#[test]
fn agent_colors_stay_readable_and_distinct_for_many_agents() {
    // WCAG luminance, independently evaluated at the persisted RGB boundary.
    let luminance = |rgb: [u8; 3]| {
        rgb.into_iter()
            .zip([0.2126, 0.7152, 0.0722])
            .map(|(channel, weight)| {
                let channel = f64::from(channel) / 255.0;
                weight
                    * if channel <= 0.04045 {
                        channel / 12.92
                    } else {
                        ((channel + 0.055) / 1.055).powf(2.4)
                    }
            })
            .sum::<f64>()
    };
    let mut state = BusState::new();
    let room = state.create_room("many agents").unwrap();
    let mut occupied = BTreeSet::from([[102, 255, 102]]);
    for _ in 0..32 {
        let id = state
            .create_agent(room, "agent", Provider::Codex, "/repo".into(), None)
            .unwrap();
        let color = serialized_agent_color(&state, id);
        assert!(occupied.insert(color), "color was reused: {color:?}");
        assert!(
            !(u16::from(color[1]) > u16::from(color[0]) + 32
                && u16::from(color[1]) > u16::from(color[2]) + 32),
            "recognizably green identity belongs to You: {color:?}"
        );
        let contrast = (luminance(color) + 0.05) / (luminance([24, 24, 28]) + 0.05);
        assert!(contrast >= 4.5, "unreadable color {color:?}: {contrast}");
    }
}

#[test]
fn agent_colors_survive_rename_neighbor_deletion_and_restart() {
    let (mut state, _, first, second) = state_with_room_and_agents();
    let original = serialized_agent_color(&state, second);
    state.rename_agent(second, "new name").unwrap();
    state.delete_agent(first).unwrap();
    let reloaded: BusState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(serialized_agent_color(&reloaded, second), original);
    assert_eq!(reloaded, state);
}

#[test]
fn legacy_agent_colors_backfill_without_recoloring_saved_neighbors() {
    let (state, _, first, second) = state_with_room_and_agents();
    let mut document = serde_json::to_value(&state).unwrap();
    document["agents"][first.0.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("color");
    // A saved custom color must be retained and considered during migration.
    document["agents"][second.0.to_string()]["color"] = serde_json::json!([0, 136, 255]);
    let migrated: BusState = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(serialized_agent_color(&migrated, second), [0, 136, 255]);
    let first_color = serialized_agent_color(&migrated, first);
    assert_ne!(first_color, [0, 136, 255]);
    assert_ne!(first_color, [102, 255, 102]);
    assert_ne!(first_color, [0, 0, 0]);
    assert_eq!(
        migrated,
        serde_json::from_value::<BusState>(document).unwrap(),
        "legacy assignment is deterministic"
    );
    assert_eq!(
        migrated,
        serde_json::from_value::<BusState>(serde_json::to_value(&migrated).unwrap()).unwrap(),
        "migration is idempotent"
    );
}

#[test]
fn agent_colors_migrate_unreadable_or_reserved_saved_rgb() {
    let (mut state, _, first, second) = state_with_room_and_agents();
    state.delete_agent(second).unwrap();
    for invalid in [[0, 0, 0], [102, 255, 102], [0, 170, 0]] {
        let mut document = serde_json::to_value(&state).unwrap();
        document["agents"][first.0.to_string()]["color"] = serde_json::json!(invalid);
        let migrated: BusState = serde_json::from_value(document).unwrap();
        assert_eq!(serialized_agent_color(&migrated, first), [221, 0, 255]);
    }
}

#[test]
fn accessible_agent_colors_persist_beside_standard_colors() {
    let (mut state, _, first, second) = state_with_room_and_agents();
    let standard = serialized_agent_color(&state, second);
    let accessible = serialized_accessible_color(&state, second);
    assert!(crate::messaging::prefs::colors::is_accessible_agent_color(
        accessible
    ));
    assert_ne!(serialized_accessible_color(&state, first), accessible);
    state.rename_agent(second, "new name").unwrap();
    state.delete_agent(first).unwrap();
    let reloaded: BusState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(serialized_agent_color(&reloaded, second), standard);
    assert_eq!(serialized_accessible_color(&reloaded, second), accessible);
    assert_eq!(reloaded, state);
}

#[test]
fn legacy_agents_backfill_accessible_colors_in_creation_order() {
    let (state, _, first, second) = state_with_room_and_agents();
    let mut document = serde_json::to_value(&state).unwrap();
    for agent in [first, second] {
        document["agents"][agent.0.to_string()]
            .as_object_mut()
            .unwrap()
            .remove("accessible_color");
    }
    let migrated: BusState = serde_json::from_value(document).unwrap();
    assert_eq!(migrated, state, "standard colors are untouched");
}

#[test]
fn deletion_cleans_owned_state_preserves_neighbors_and_never_reuses_ids() {
    let (mut state, room, agent, other) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "delete active work");
    start_request(&mut state, request, "launch-codex", 10);
    let queued = submit_text(&mut state, room, agent, "delete queue");
    state.set_draft_recipients(room, [agent, other]).unwrap();
    state.rooms.get_mut(&room).unwrap().latest_replies.insert(
        agent,
        Reply {
            request_id: request,
            agent_id: agent,
            text: "old reply".into(),
            received_at_ms: 1,
        },
    );
    let unrelated_room = state.create_room("unrelated").unwrap();
    let unaffected = state.agent(other).unwrap().clone();
    state.delete_agent(agent).unwrap();
    assert_eq!(state.agent(other), Some(&unaffected));
    assert!(state.request(request).is_none());
    assert!(state.request(queued).is_none());
    assert!(state.queued_requests(agent).is_empty());
    assert_eq!(
        state.room(room).unwrap().draft.recipient_ids,
        [other].into()
    );
    assert!(state.room(room).unwrap().latest_replies.is_empty());
    assert!(matches!(
        state.accept_callback(ProviderCallback::final_event(
            "late",
            99,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "delete active work",
            "late result"
        )),
        CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest)
    ));
    state.select_room(room).unwrap();
    state.delete_room(room).unwrap();
    assert!(state.agent(other).is_none());
    let fallback = state.rooms().next().map(|room| room.id);
    assert!(fallback.is_some());
    assert_eq!(state.visible_room, fallback);
    assert!(state.room(unrelated_room).is_some());
    let new_room = state.create_room("new").unwrap();
    assert!(new_room.0 > unrelated_room.0);
    assert!(!state.is_pristine());
    state.delete_room(new_room).unwrap();
    state.delete_room(unrelated_room).unwrap();
    assert!(!state.is_pristine());
}
