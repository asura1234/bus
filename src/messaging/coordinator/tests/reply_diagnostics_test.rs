use super::*;

fn reply(f: &mut Fixture, message: PromptId, agent: AgentId, text: &str) -> Response {
    f.worker.control_response_with_events(
        &ControlRequest {
            id: format!("reply-diagnostics-{}", NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)),
            method: "message.send".into(),
            params: json!({"reply_to": message.0.to_string(), "as": agent.0.to_string(), "text": text}),
        },
        None,
    )
}

#[test]
fn reply_diagnostics_captured_final_explains_the_failed_corrected_reply() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "verify");
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    let before = f.worker.state.clone();
    let hybrid = f.worker.control_response_with_events(&ControlRequest {
        id: "invalid-hybrid".into(),
        method: "message.send".into(),
        params: json!({"reply_to":message.0.to_string(),"as":agent.0.to_string(),"text":"VERIFIED","room":f.room.0.to_string(),"to":["orchestrator"]}),
    }, None);
    assert!(!hybrid.ok);
    assert_eq!(
        f.worker.state, before,
        "a rejected hybrid must not fill the reply slot"
    );

    f.stopped("infinite-turn", "I verified the change; App pending.");
    f.consume();
    let captured_at = f.request(lead).reply().unwrap().received_at_ms;
    let before = f.worker.state.clone();
    let response = reply(&mut f, message, agent, "VERIFIED; App pending.");
    assert!(!response.ok);
    let error = response.error.unwrap();
    assert_eq!(error.code, "request_already_replied");
    assert!(
        error.message.contains("new text was not sent"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("already complete"),
        "{}",
        error.message
    );
    assert!(error
        .message
        .contains(&format!("bus message status {}", message.0)));
    assert_eq!(response.result["message_id"], message.0);
    assert_eq!(response.result["request_ids"], json!([lead.0]));
    assert_eq!(response.result["request_complete"], true);
    assert_eq!(response.result["new_text_sent"], false);
    assert_eq!(
        response.result["existing_reply"]["source"],
        "captured_turn_final"
    );
    assert_eq!(
        response.result["existing_reply"]["received_at_ms"],
        captured_at
    );
    assert_eq!(
        response.result["existing_reply"]["preview"],
        "I verified the change; App pending."
    );
    assert_eq!(
        f.worker.state, before,
        "diagnostics must preserve the first answer"
    );
    assert!(
        reply(
            &mut f,
            message,
            agent,
            "I verified the change; App pending."
        )
        .ok
    );
    assert_eq!(
        f.worker.state, before,
        "the captured answer's exact-text retry stays idempotent"
    );
}

#[test]
fn reply_diagnostics_recipient_reply_survives_stop_and_bounds_unicode_preview() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "verify");
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    let text = "验🙂".repeat(200);
    assert!(reply(&mut f, message, agent, &text).ok);
    let received_at = f.request(lead).reply().unwrap().received_at_ms;
    f.stopped("infinite-turn", "Later final must not replace the answer");
    f.consume();
    let before = f.worker.state.clone();
    let response = reply(&mut f, message, agent, "different answer");
    assert_eq!(response.error.unwrap().code, "request_already_replied");
    assert_eq!(
        response.result["existing_reply"]["source"],
        "recipient_reply"
    );
    assert_eq!(
        response.result["existing_reply"]["received_at_ms"],
        received_at
    );
    let preview = response.result["existing_reply"]["preview"]
        .as_str()
        .unwrap();
    assert_eq!(preview, format!("{}…", "验🙂".repeat(80)));
    assert_eq!(preview.chars().count(), 161);
    assert_eq!(f.worker.state, before);
    assert!(reply(&mut f, message, agent, &text).ok);
    assert_eq!(f.worker.state, before);
}

#[test]
fn reply_diagnostics_do_not_disclose_existing_answers_to_other_agents() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "verify");
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    f.stopped("infinite-turn", "private answer");
    f.consume();
    let other = f
        .worker
        .state
        .create_agent(f.room, "other", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    let response = reply(&mut f, message, other, "different");
    assert_eq!(response.error.unwrap().code, "command_failed");
    assert!(response.result.is_null());
    assert!(!serde_json::to_string(&response.result)
        .unwrap()
        .contains("private answer"));
    assert_eq!(f.request(lead).reply().unwrap().text, "private answer");
    assert!(reply(&mut f, message, agent, "private answer").ok);
}

#[test]
fn reply_diagnostics_each_delivered_hint_explains_receipt_and_recovery() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "verify");
    let message = f.request(lead).prompt.id;
    let payload = f.typed().last().unwrap().0.clone();
    assert!(payload.contains("foreground"), "{payload}");
    assert!(payload.contains("stage:\"replied\""), "{payload}");
    assert!(
        payload.contains(&format!("bus message status {}", message.0)),
        "{payload}"
    );
    assert!(payload.contains("before resending"), "{payload}");
    assert_eq!(test_prompt_content(&payload), "verify");
}

#[test]
fn reply_diagnostics_joined_request_answered_by_group_final_reports_captured_source() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "standing watcher");
    let next = start(&mut f, "one task");
    assert_eq!(f.request(next).group, Some(lead));
    let message = f.request(next).prompt.id;
    let agent = f.agent;
    f.stopped("infinite-turn", "group final");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.worker.settle_requests().unwrap();
    let before = f.worker.state.clone();
    let response = reply(&mut f, message, agent, "late corrected answer");
    assert_eq!(response.error.unwrap().code, "request_already_replied");
    assert_eq!(response.result["request_ids"], json!([next.0]));
    assert_eq!(
        response.result["existing_reply"]["source"],
        "captured_turn_final"
    );
    assert_eq!(response.result["existing_reply"]["preview"], "group final");
    assert_eq!(f.worker.state, before);
    assert!(reply(&mut f, message, agent, "group final").ok);
    assert_eq!(f.worker.state, before);
}

#[test]
fn reply_diagnostics_preview_secrets_stay_out_of_error_text_and_logs() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "verify");
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    let secret = "token=synthetic-secret-\"\\\n\u{1b}[31m验🙂";
    assert!(reply(&mut f, message, agent, secret).ok);
    let capture = crate::utils::logging::test_capture::Capture::default();
    let mut captured = None;
    capture.run_filtered("trace", || {
        captured = Some(reply(&mut f, message, agent, "correction"))
    });
    let response = captured.unwrap();
    assert!(!response.ok);
    assert_eq!(response.result["existing_reply"]["preview"], secret);
    assert!(!response
        .error
        .as_ref()
        .unwrap()
        .message
        .contains("synthetic-secret"));
    assert!(
        !capture.text().contains("synthetic-secret"),
        "{}",
        capture.text()
    );
    let wire = serde_json::to_string(&response).unwrap();
    assert!(!wire.contains('\u{1b}'));
    assert_eq!(wire.lines().count(), 1);
    let decoded: Response = serde_json::from_str(&wire).unwrap();
    assert_eq!(decoded.result["existing_reply"]["preview"], secret);
}

#[test]
fn reply_diagnostics_guidance_and_first_answer_work_for_each_provider() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let mut f = Fixture::new(provider);
        let lead = start(&mut f, "verify");
        let message = f.request(lead).prompt.id;
        let agent = f.agent;
        let payload = f.typed().last().unwrap().0.clone();
        assert!(
            payload.contains("foreground")
                && payload.contains("ok:true")
                && payload.contains("stage:\"replied\""),
            "{provider:?}: {payload}"
        );
        assert!(payload.contains(&format!(
            "bus message status {} before resending",
            message.0
        )));
        assert!(reply(&mut f, message, agent, "first answer").ok);
        f.stopped("infinite-turn", "later final");
        f.consume();
        let before = f.worker.state.clone();
        let response = reply(&mut f, message, agent, "correction");
        assert_eq!(
            response.error.unwrap().code,
            "request_already_replied",
            "{provider:?}"
        );
        assert_eq!(
            response.result["existing_reply"]["source"],
            "recipient_reply"
        );
        assert_eq!(response.result["existing_reply"]["preview"], "first answer");
        assert_eq!(response.result["new_text_sent"], false);
        assert!(reply(&mut f, message, agent, "first answer").ok);
        assert_eq!(f.worker.state, before, "{provider:?}");
    }
}
