use super::*;

fn payload_from(w: &mut Worker, room: RoomId, sender: AgentId, peer: AgentId) -> String {
    let id = w
        .state
        .submit_message_from(
            room,
            crate::messaging::model::Draft {
                text: "hello".into(),
                files: vec![],
                recipient_ids: [peer].into(),
            },
            Author::Agent(sender),
            1,
        )
        .unwrap()[0];
    w.state.render_request_payload(id).unwrap()
}

/// A header name must lead back to its sender: replying with the name shown
/// must not reach a different agent whose numeric ID equals that name.
#[test]
fn selector_label_numeric_name_header_resolves_back_to_its_sender() {
    let (mut w, room, _agent, dir) = fixture();
    let other = w.state.create_room("other").unwrap();
    let elsewhere = w
        .state
        .create_agent(other, "elsewhere", Provider::Codex, dir.clone(), None)
        .unwrap();
    let sender = w
        .state
        .create_agent(
            room,
            &elsewhere.0.to_string(),
            Provider::Codex,
            dir.clone(),
            None,
        )
        .unwrap();
    let peer = w
        .state
        .create_agent(room, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    let payload = payload_from(&mut w, room, sender, peer);
    let header = payload.lines().next().unwrap().to_owned();
    // The selector a reader would copy from the header.
    let shown = header
        .trim_end_matches(':')
        .split(" (")
        .next()
        .unwrap()
        .to_owned();
    let resolved = w
        .scoped_agent(&json!({"agent": shown, "room": room.0.to_string()}))
        .unwrap();
    assert!(
        resolved == sender || header.contains(&format!("agent {}", sender.0)),
        "header {header:?} names {shown:?}, which resolves to agent {} not sender {}",
        resolved.0,
        sender.0
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The selector Bus embeds in an actionable notice resolves to that agent
/// with no --room/--as context, as the notice's reader would run it.
#[test]
fn selector_label_cli_selector_in_notices_round_trips_without_context() {
    let (mut w, room, _agent, dir) = fixture();
    let other = w.state.create_room("other").unwrap();
    let names = ["plain", "o'brien", "-dash", "12", "shared", "with space"];
    let mut ids = Vec::new();
    for name in names {
        ids.push(
            w.state
                .create_agent(room, name, Provider::Codex, dir.clone(), None)
                .unwrap(),
        );
    }
    w.state
        .create_agent(other, "shared", Provider::Codex, dir.clone(), None)
        .unwrap();
    for id in ids {
        let selector = w.state.agent_cli_selector(id);
        // Undo the shell quoting the notice applies.
        let unquoted = selector
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .map_or(selector.clone(), |s| s.replace("'\\''", "'"));
        assert_eq!(
            w.scoped_agent(&json!({"agent": unquoted})),
            Ok(id),
            "selector {selector:?}"
        );
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

/// `--as` only supplies the sender's home room for name resolution; an exact
/// target ID needs no such context, so an ambiguous --as must not block it.
#[test]
fn selector_label_ambiguous_as_does_not_block_an_exact_id_target() {
    let (mut w, room, agent, dir) = fixture();
    let other = w.state.create_room("other").unwrap();
    w.state
        .create_agent(other, "codex1", Provider::Codex, dir.clone(), None)
        .unwrap();
    let _ = room;
    for author in ["codex1", "ghost-author"] {
        let result = w.scoped_agent(&json!({"agent": agent.0.to_string(), "as": author}));
        assert_eq!(result, Ok(agent));
    }
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn selector_label_numeric_names_include_ids_without_an_id_collision() {
    let (mut w, room, peer, dir) = fixture();
    let sender = w
        .state
        .create_agent(room, "999999", Provider::Codex, dir.clone(), None)
        .unwrap();
    assert_eq!(
        w.state.agent_label(sender),
        format!("999999 (agent {})", sender.0)
    );
    assert!(payload_from(&mut w, room, sender, peer)
        .starts_with(&format!("999999 (agent {}):\n", sender.0)));
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}
