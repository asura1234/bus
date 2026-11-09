use super::*;

fn history(f: &mut Fixture) -> Value {
    let response = f.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "history".into(),
            method: "room.history".into(),
            params: json!({"room": "room"}),
        },
        None,
    );
    assert!(response.ok, "{response:?}");
    response.result
}

/// The turn's final text is not a reply, and the message queued behind it
/// is delivered once that turn ends.
#[test]
fn a_turn_final_settles_without_a_reply_and_delivers_the_queued_message() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "do the work", "turn-1");
    let queued = fixture.send("next task", true);
    fixture.deliver();
    assert_eq!(
        fixture.typed_content(),
        [("do the work".to_owned(), false)],
        "a queued message must wait for the current turn"
    );

    fixture.stopped("turn-1", "FINAL_TEXT_SHOULD_NOT_LEAK");
    fixture.consume();
    assert!(
        fixture.request(lead).settled(),
        "phase={:?}",
        fixture.request(lead).phase
    );
    assert!(fixture.reply(lead).is_none(), "{:?}", fixture.reply(lead));
    assert!(fixture
        .worker
        .state
        .room(fixture.room)
        .unwrap()
        .latest_replies
        .is_empty());

    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    assert!(
        fixture
            .typed_content()
            .iter()
            .any(|(text, steer)| text == "next task" && !steer),
        "queued request {} was not delivered after the turn ended: {:?}",
        queued.0,
        fixture.typed_content()
    );
    let stored = fixture.worker.state.requests().any(|request| {
        request.prompt.text.contains("FINAL_TEXT_SHOULD_NOT_LEAK")
            || request.prompt.text.contains("Reply to message")
    }) || fixture.worker.state.notices(fixture.room).any(|notice| {
        notice.text.contains("FINAL_TEXT_SHOULD_NOT_LEAK")
            || notice.text.contains("Reply to message")
    });
    assert!(!stored, "the turn final was stored as a reply");
}

/// History is a list of messages. It does not show a reply notice, reply
/// status, or group/reply badge for a delivered message.
#[test]
fn history_omits_reply_notice_and_reply_status_for_a_delivered_message() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "do the work", "turn-1");
    fixture.stopped("turn-1", "FINAL_TEXT_SHOULD_NOT_LEAK");
    fixture.consume();
    let history = history(&mut fixture);
    let rendered = history.to_string();
    assert!(
        !rendered.contains("Reply to message"),
        "history contains a reply notice: {rendered}"
    );
    assert!(
        !rendered.contains("FINAL_TEXT_SHOULD_NOT_LEAK"),
        "history contains the captured final: {rendered}"
    );
    assert!(
        !rendered.contains("answered with message"),
        "history contains a group/reply badge: {rendered}"
    );
    let message = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["prompt"]["id"] == json!(fixture.request(lead).prompt.id))
        .expect("delivered message");
    let requests = message["delivery"]["requests"].as_array().unwrap();
    assert!(!requests.is_empty());
    for request in requests {
        assert!(request["reply"].is_null(), "{request}");
        assert!(request.get("group").is_none() || request["group"].is_null());
        assert!(request.get("answered_with_message").is_none());
        assert_ne!(request["stage"], "replied", "{request}");
        assert_ne!(request["reply_ack"], true, "{request}");
    }
}

/// An explicit reply settles the request while its turn is still open.
#[test]
fn explicit_reply_to_settles_before_the_turn_ends() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "question", "turn-1");
    let message = fixture.request(lead).prompt.id;
    let agent = fixture.agent;
    let sent = fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "reply".into(),
            method: "message.send".into(),
            params: json!({"reply_to": message.0.to_string(), "as": agent.0.to_string(), "text": "explicit answer"}),
        },
        None,
    );
    assert!(sent.ok, "{sent:?}");
    assert_eq!(sent.result["stage"], "replied");
    assert!(fixture.request(lead).settled());
    assert_eq!(fixture.reply(lead).as_deref(), Some("explicit answer"));
    assert_eq!(
        fixture.worker.state.agent(agent).unwrap().current_request,
        Some(lead)
    );
}

/// History keeps a normal message and does not render an explicit reply as a reply row.
#[test]
fn history_shows_the_normal_message_and_strips_reply_status() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "question", "turn-1");
    let message = fixture.request(lead).prompt.id;
    let agent = fixture.agent;
    let sent = fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "reply".into(),
            method: "message.send".into(),
            params: json!({
                "reply_to": message.0.to_string(),
                "as": agent.0.to_string(),
                "text": "explicit answer"
            }),
        },
        None,
    );
    assert!(sent.ok, "{sent:?}");
    let plain = fixture.send("plain status", true);
    fixture.stopped("turn-1", "FINAL_TEXT_SHOULD_NOT_LEAK");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    let history = history(&mut fixture);
    let rendered = history.to_string();
    assert!(
        !rendered.contains("Reply to message"),
        "history contains a reply notice: {rendered}"
    );
    assert!(
        !rendered.contains("FINAL_TEXT_SHOULD_NOT_LEAK"),
        "history contains the turn final: {rendered}"
    );
    assert!(rendered.contains("question"), "{rendered}");
    assert!(rendered.contains("plain status"), "{rendered}");
    let prompt_id = json!(fixture.request(lead).prompt.id);
    let prompts: Vec<_> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["prompt"]["text"].as_str())
        .collect();
    assert!(
        !prompts.iter().any(|text| text.contains("explicit answer")),
        "an explicit reply is not its own history message: {prompts:?}"
    );
    let original = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["prompt"]["id"] == prompt_id)
        .expect("original message");
    for request in original["delivery"]["requests"].as_array().unwrap() {
        assert!(request["reply"].is_null(), "{request}");
        assert_ne!(request["stage"], "replied", "{request}");
        assert!(request.get("answered_with_message").is_none());
    }
    assert_eq!(fixture.request(plain).phase, RequestPhase::Queued);
}

/// A coalesced group settles with the turn and the message behind it is delivered.
#[test]
fn a_coalesced_group_settles_when_the_turn_ends_and_the_queue_advances() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.status(RuntimeStatus::Launching);
    let first = fixture.send("first", false);
    let second = fixture.send("second", false);
    let own_turn = fixture.send("own turn", true);
    fixture.deliver();
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    let typed = fixture.typed_content();
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert_eq!(fixture.request(second).group, Some(first));
    fixture.started("turn-1", &typed[0].0);
    fixture.stopped("turn-1", "both done");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert!(fixture.request(first).settled());
    assert!(fixture.request(second).settled());
    assert!(fixture.reply(first).is_none(), "{:?}", fixture.reply(first));
    assert!(
        fixture.reply(second).is_none(),
        "{:?}",
        fixture.reply(second)
    );
    assert_ne!(
        fixture.request(first).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    assert_ne!(
        fixture.request(second).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    fixture.deliver();
    assert!(
        fixture
            .typed_content()
            .iter()
            .any(|(text, steer)| text == "own turn" && !steer),
        "queued request {} was not delivered: {:?}",
        own_turn.0,
        fixture.typed_content()
    );
}
