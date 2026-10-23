use super::*;

#[test]
fn identity_scoped_agent_commands_use_conversation_then_home_then_global() {
    struct Refusing;
    impl Transport for Refusing {
        fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
            Err(TransportError {
                message: "no terminal in test".into(),
                code: None,
                definitely_rejected: true,
            })
        }
    }
    for (index, method) in [
        "agent.read",
        "agent.dialog.observe",
        "agent.dialog.choose",
        "agent.dialog.answer",
        "agent.clear",
        "agent.rename",
        "agent.setup-confirm",
        "agent.delete",
        "agent.interrupt",
        "agent.focus",
        "agent.details",
    ]
    .into_iter()
    .enumerate()
    {
        let (mut w, room, agent, dir) = fixture();
        w.transport = Box::new(Refusing);
        let other = w.state.create_room("other").unwrap();
        let twin = w
            .state
            .create_agent(other, "codex1", Provider::Codex, dir.clone(), None)
            .unwrap();
        let params = json!({"agent":"codex1", "room":room.0.to_string(), "confirm":true, "name":"codex1", "on":true, "option":"1", "fingerprint":"invalid", "text":"answer", "source":"visible"});
        let fields = method_spec(method).unwrap().fields;
        let mut by_name = params.as_object().unwrap().clone();
        by_name.retain(|key, _| fields.contains(&key.as_str()) || key == "room");
        let result = call(&mut w, &format!("name-{index}"), method, by_name.into());
        let error = error_message(&result);
        assert_ne!(error, "Unknown parameter", "{method}");
        assert!(
            !error.contains("Ambiguous")
                && !error.contains("No matching")
                && !error.contains("No agent matches"),
            "{method}: {result:?}"
        );
        assert!(
            w.state.agent(twin).is_some(),
            "must not operate on the foreign twin"
        );
        if method == "agent.delete" {
            assert!(w.state.agent(agent).is_none());
        }
        drop(w);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn identity_numeric_ids_win_over_numeric_names_for_all_agent_commands() {
    let (mut w, room, agent, dir) = fixture();
    w.state
        .create_agent(
            room,
            &agent.0.to_string(),
            Provider::Codex,
            dir.clone(),
            None,
        )
        .unwrap();
    let result = call(
        &mut w,
        "numeric",
        "agent.details",
        json!({"agent":agent.0.to_string(),"on":true}),
    );
    assert!(result.ok, "{result:?}");
    assert!(w.state.agent(agent).unwrap().details_disclosed);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn identity_agent_list_exposes_name_id_and_room() {
    let (mut w, room, agent, dir) = fixture();
    let result = call(
        &mut w,
        "list",
        "agent.list",
        json!({"room":room.0.to_string()}),
    );
    assert!(result.ok, "{result:?}");
    assert_eq!(
        result.result["agents"][0],
        json!({"name":"codex1","id":agent,"room":room})
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn identity_shared_resolver_falls_back_to_sender_home_and_lists_scoped_candidates() {
    let (mut w, home, sender, dir) = fixture();
    let conversation = w.state.create_room("conversation").unwrap();
    let elsewhere = w.state.create_room("elsewhere").unwrap();
    let home_peer = w
        .state
        .create_agent(home, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    let remote_peer = w
        .state
        .create_agent(elsewhere, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    let context = json!({"room":conversation.0.to_string(),"as":"codex1","agent":"peer"});
    assert_eq!(w.scoped_agent(&context).unwrap(), home_peer);
    let local_peer = w
        .state
        .create_agent(conversation, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    assert_eq!(w.scoped_agent(&context).unwrap(), local_peer);
    let duplicate = w
        .state
        .create_agent(conversation, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    let error = w.scoped_agent(&context).unwrap_err();
    for id in [local_peer, duplicate] {
        assert!(
            error.contains(&format!("agent {}, room {}", id.0, conversation.0)),
            "{error}"
        );
    }
    assert!(
        !error.contains(&format!("agent {}, room {}", remote_peer.0, elsewhere.0)),
        "{error}"
    );
    let numeric = json!({"room":conversation.0.to_string(),"as":sender.0.to_string(),"agent":remote_peer.0.to_string()});
    assert_eq!(w.scoped_agent(&numeric).unwrap(), remote_peer);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn identity_headers_use_names_and_disambiguate_only_shared_names() {
    let (mut w, room, sender, dir) = fixture();
    let peer = w
        .state
        .create_agent(room, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    let send_payload = |w: &mut Worker, text: &str| {
        let id = w
            .state
            .submit_message_from(
                room,
                crate::messaging::model::Draft {
                    text: text.into(),
                    files: vec![],
                    recipient_ids: [peer].into(),
                },
                Author::Agent(sender),
                1,
            )
            .unwrap()[0];
        w.state.render_request_payload(id).unwrap()
    };
    assert_eq!(send_payload(&mut w, "hello"), "codex1:\nhello");
    w.state
        .create_agent(room, "codex1", Provider::Codex, dir.clone(), None)
        .unwrap();
    assert_eq!(
        send_payload(&mut w, "codex1:\nhello"),
        format!("codex1 (agent {}):\nhello", sender.0)
    );
    assert_eq!(
        send_payload(&mut w, "codex1 (room 999):\nhello"),
        format!("codex1 (agent {}):\ncodex1 (room 999):\nhello", sender.0)
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn identity_unresolved_agent_selector_errors_name_the_failing_selector() {
    let (mut w, _room, _agent, dir) = fixture();
    let bad_author = call(
        &mut w,
        "bad-as",
        "agent.dialog.observe",
        json!({"agent":"codex1","as":"ghost-author"}),
    );
    let bad_target = call(
        &mut w,
        "bad-agent",
        "agent.dialog.observe",
        json!({"agent":"ghost-target","as":"codex1"}),
    );
    let author_error = error_message(&bad_author);
    let target_error = error_message(&bad_target);
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        author_error.contains("ghost-author"),
        "--as failure must name the --as selector: {author_error:?}"
    );
    assert!(author_error.starts_with("--as:"), "{author_error}");
    assert!(
        target_error.contains("ghost-target"),
        "AGENT failure must name the AGENT selector: {target_error:?}"
    );
}

#[test]
fn identity_interrupt_preserves_receipt_context_and_target_room_notice() {
    let (mut w, home, target, dir) = fixture();
    let conversation = w.state.create_room("conversation").unwrap();
    w.state
        .create_agent(home, "owner", Provider::Codex, dir.clone(), None)
        .unwrap();
    let author = w
        .state
        .create_agent(conversation, "owner", Provider::Codex, dir.clone(), None)
        .unwrap();
    w.state
        .observe_status(target, RuntimeStatus::Idle, 1)
        .unwrap();
    let request = w
        .state
        .submit_message_from(
            home,
            Draft {
                text: "queued work".into(),
                files: vec![],
                recipient_ids: [target].into(),
            },
            Author::Developer,
            2,
        )
        .unwrap()[0];
    let response = call(
        &mut w,
        "interrupt-context",
        "agent.interrupt",
        json!({"agent":target.0.to_string(),"room":conversation.0.to_string(),"as":"owner"}),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.result["conversation_room_id"], json!(conversation));
    assert_eq!(response.result["author_id"], json!(author));
    assert_eq!(
        w.state.room(home).unwrap().notices.last().unwrap().author,
        Author::Agent(author)
    );
    assert!(w.state.room(conversation).unwrap().notices.is_empty());
    assert_eq!(
        w.state.request(request).unwrap().failure_reason.as_deref(),
        Some("interrupted")
    );
    drop(w);
    std::fs::remove_dir_all(dir).unwrap();
}
