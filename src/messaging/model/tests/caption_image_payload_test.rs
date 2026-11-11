//! A real Claude caption/path transformation must still prove owned input.
use super::*;

fn caption_fixture() -> (String, String) {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/claude_caption_image.json")).unwrap();
    assert_eq!(data["image_blocks"], 1);
    (
        data["typed_body"].as_str().unwrap().to_owned(),
        data["native_prompt"].as_str().unwrap().to_owned(),
    )
}

fn caption_prompt(raw: &str) -> String {
    let crate::agents::providers::spool::Parsed::Started { prompt, .. } =
        crate::agents::providers::spool::parse(
            Provider::ClaudeCode,
            &serde_json::json!({
                "hook_event_name":"UserPromptSubmit", "session_id":"provider-session",
                "prompt_id":"turn-1", "prompt":raw,
            }),
        )
        .unwrap()
    else {
        panic!("expected the captured native start");
    };
    prompt
}

fn caption_worker(provider: Provider) -> (BusState, RoomId, AgentId) {
    let mut state = BusState::default();
    let room = state.create_room("caption").unwrap();
    let agent = state
        .create_agent(room, "worker", provider, "/repo".into(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("caption-launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    (state, room, agent)
}

#[test]
fn caption_png_capture_preserves_exact_owned_body_header_and_image_count() {
    let (body, raw) = caption_fixture();
    let typed = format!("[Bus message 6 from you]\n{body}");
    let prompt = caption_prompt(&raw);
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        assert!(super::super::payload::payload_matches_for_provider(
            &prompt, &typed, provider
        ));
        for foreign in [
            prompt.replace("message 6", "message 7"),
            prompt.replace("report detail 20:", "report detail 21:"),
            prompt.replace("Dialog screenshot:", "Different screenshot:"),
            format!("[Image #4]{prompt}"),
            prompt.replacen("[Image #3]", "", 1),
            format!("{prompt}\nextra"),
        ] {
            assert!(!super::super::payload::payload_matches_for_provider(
                &foreign, &typed, provider
            ));
        }
    }
}

#[test]
fn caption_png_callback_binds_its_frozen_turn_and_releases_the_follower() {
    let (body, raw) = caption_fixture();
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let (mut state, room, agent) = caption_worker(provider);
        let request = submit_text(&mut state, room, agent, &body);
        let follower = submit_text(&mut state, room, agent, "queued follower");
        let id = state.request(request).unwrap().prompt.id.0;
        let frozen = state.render_request_payload(request).unwrap();
        submit_request(&mut state, request, "caption-launch", 10);
        let prompt = caption_prompt(&raw.replace("message 6", &format!("message {id}")));
        let start = ProviderCallback {
            callback_id: "caption-start".into(),
            sequence: 11,
            occurred_at_ms: 11,
            agent_id: agent,
            launch_id: "caption-launch".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: Some("turn-1".into()),
            prompt_payload: Some(prompt.clone()),
            kind: CallbackEventKind::PromptStarted,
        };
        let mut foreign = start.clone();
        foreign.callback_id = "caption-foreign-launch".into();
        foreign.launch_id = "other-launch".into();
        assert!(matches!(
            state.accept_callback(foreign),
            CallbackDisposition::Rejected(_)
        ));
        assert_eq!(
            state.accept_callback(start),
            CallbackDisposition::AcceptedBinding
        );
        assert_eq!(
            state.accept_callback(ProviderCallback::final_event(
                "caption-final",
                12,
                agent,
                "caption-launch",
                "provider-session",
                "turn-1",
                &prompt,
                "STASH_4136a0ecb1",
            )),
            CallbackDisposition::AcceptedCompleted
        );
        state
            .observe_status(agent, RuntimeStatus::Idle, 13)
            .unwrap();
        let settled = state.request(request).unwrap();
        assert!(settled.trusted_start_bound && state.turn_ended(settled));
        assert_eq!(settled.phase, RequestPhase::Completed);
        assert!(settled.failure_reason.is_none() && settled.reply().is_none());
        assert_eq!(settled.provider_turn_id.as_deref(), Some("turn-1"));
        assert_eq!(settled.submitted_payload.as_deref(), Some(frozen.as_str()));
        assert_eq!(settled.prompt.text, body);
        assert_eq!(state.next_queued_request(agent), Some(follower));
    }
}

#[test]
fn caption_png_is_literal_for_every_provider_and_foreign_to_non_claude() {
    let (body, raw) = caption_fixture();
    for provider in [
        Provider::ClaudeCode,
        Provider::ClaudeDs,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        let (mut state, room, agent) = caption_worker(provider);
        let request = submit_text(&mut state, room, agent, &body);
        let id = state.request(request).unwrap().prompt.id.0;
        let frozen = state.render_request_payload(request).unwrap();
        submit_request(&mut state, request, "caption-launch", 10);
        assert!(state
            .request(request)
            .unwrap()
            .matches_callback_payload(&frozen, provider));
        if matches!(provider, Provider::ClaudeCode | Provider::ClaudeDs) {
            continue;
        }
        let prompt = caption_prompt(&raw.replace("message 6", &format!("message {id}")));
        assert_eq!(
            state.accept_callback(ProviderCallback {
                callback_id: "caption-foreign-substitution".into(),
                sequence: 11,
                occurred_at_ms: 11,
                agent_id: agent,
                launch_id: "caption-launch".into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
                provider_prompt_id: Some("turn-1".into()),
                prompt_payload: Some(prompt),
                kind: CallbackEventKind::PromptStarted,
            }),
            CallbackDisposition::Rejected(CallbackRejection::WrongPrompt)
        );
        assert!(!state.request(request).unwrap().trusted_start_bound);
    }
}

#[test]
fn caption_png_mixed_read_results_account_for_each_exact_literal_path() {
    let typed = "[Bus message 6 from you]\nReport\nDialog screenshot: /tmp/a.png\n\"/tmp/b.jpeg\"";
    for remaining in [
        "Dialog screenshot: /tmp/a.png",
        "Dialog screenshot:\n\"/tmp/b.jpeg\"",
    ] {
        let payload = format!("[Image #3][Bus message 6 from you]\nReport\n{remaining}");
        assert!(payload_matches(&payload, typed));
        assert!(!payload_matches(
            &payload.replace("/tmp/", "/foreign/"),
            typed
        ));
        assert!(!payload_matches(&format!("[Image #4]{payload}"), typed));
    }
}
