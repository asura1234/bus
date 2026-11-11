//! Provider image lifting must keep the entire owned text and callback identity.
use super::*;

#[test]
fn claude_unquoted_image_path_matches_its_exact_remaining_payload() {
    let body = "[Bus message 6 from you]\nReply with only IMAGE_CORRELATION_MARKER.\n  Keep internal \t spacing.";
    for path in ["/private/tmp/bus-test/fixture.png", "/tmp/图片.PNG"] {
        let typed = format!("{body}\n{path}");
        let accepted = format!("[Image #3]{body}");
        assert!(
            payload_matches(&accepted, &typed),
            "unquoted image was lifted: {path}"
        );
        for wrong in [
            accepted.replace("message 6", "message 7"),
            accepted.replace("  Keep", " Keep"),
            format!("[Image #4]{accepted}"),
            body.to_owned(),
            format!("{accepted}\nextra"),
        ] {
            assert!(
                !payload_matches(&wrong, &typed),
                "foreign or changed prompt: {wrong}"
            );
        }
    }
    for literal in [
        "fixture.png",
        "https://example.com/fixture.png",
        "/tmp/fixture.pdf",
        "/tmp/a.png /tmp/b.png",
        "Look at /tmp/a.png",
        "/tmp/one image.png",
        "/tmp/one\timage.png",
        "/tmp/one\u{a0}image.png",
        "/tmp/one\"image.png",
        "/tmp/one'image.png",
        "/tmp/one\\image.png",
        "/tmp/fixture.jpg",
    ] {
        assert!(
            !payload_matches(&format!("[Image #3]{body}"), &format!("{body}\n{literal}")),
            "not one unquoted image path: {literal}"
        );
    }
    assert!(payload_matches(
        &format!("[Image #3][Image #4]{body}"),
        &format!("{body}\n/private/tmp/a.png\n\"/tmp/another image.png\""),
    ));
}

#[test]
fn claude_unquoted_image_mixed_read_results_keep_each_exact_path() {
    let body = "[Bus message 6 from you]\nInspect both images.";
    let typed = format!("{body}\n/tmp/a.png\n\"/tmp/b.jpg\"");
    for unread in ["/tmp/a.png", "\"/tmp/b.jpg\""] {
        assert!(payload_matches(
            &format!("[Image #3]{body}\n{unread}"),
            &typed
        ));
        assert!(!payload_matches(
            &format!("[Image #3][Image #4]{body}\n{unread}"),
            &typed
        ));
        assert!(!payload_matches(
            &format!("[Image #3]{body}\n{unread}\n{unread}"),
            &typed
        ));
    }
    assert!(!payload_matches(
        &format!("[Image #3]{body}\n/tmp/foreign.png"),
        &typed
    ));
}

#[test]
fn claude_unquoted_image_callback_binds_and_ends_only_its_frozen_request() {
    for framed in [false, true] {
        let (mut state, room, _, claude) = state_with_room_and_agents();
        let body = "Reply with only IMAGE_CORRELATION_MARKER. Do not use tools.";
        let typed = format!("{body}\n/private/tmp/bus-test/fixture.png");
        let request = submit_text(&mut state, room, claude, &typed);
        let queued = submit_text(&mut state, room, claude, "queued next");
        let id = state.request(request).unwrap().prompt.id.0;
        let frozen = state.render_request_payload(request).unwrap();
        submit_request(&mut state, request, "launch-claude", 10);
        let remaining = format!("[Bus message {id} from you]\n{body}");
        let raw = if framed {
            format!("[Image #3]\n\n<pasted_content id=\"a1\">\n{remaining}\n</pasted_content id=\"a1\">\n")
        } else {
            format!("[Image #3]{remaining}")
        };
        let crate::agents::providers::spool::Parsed::Started { prompt, .. } =
            crate::agents::providers::spool::parse(
                Provider::ClaudeCode,
                &serde_json::json!({
                    "hook_event_name":"UserPromptSubmit", "session_id":"provider-session",
                    "prompt_id":"prompt-1", "prompt":raw,
                }),
            )
            .unwrap()
        else {
            panic!("expected a native start");
        };
        let start = |key: &str, payload: String| ProviderCallback {
            callback_id: key.into(),
            sequence: 11,
            occurred_at_ms: 11,
            agent_id: claude,
            launch_id: "launch-claude".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: Some("prompt-1".into()),
            prompt_payload: Some(payload),
            kind: CallbackEventKind::PromptStarted,
        };
        let mut foreign_launch = start("foreign-launch", prompt.clone());
        foreign_launch.launch_id = "other-launch".into();
        assert!(matches!(
            state.accept_callback(foreign_launch),
            CallbackDisposition::Rejected(_)
        ));
        for (key, wrong) in [
            (
                "other-message",
                prompt.replace(&format!("message {id}"), &format!("message {}", id + 1)),
            ),
            ("other-text", format!("{prompt} extra")),
            ("other-images", format!("[Image #4]{prompt}")),
        ] {
            assert_eq!(
                state.accept_callback(start(key, wrong)),
                CallbackDisposition::Rejected(CallbackRejection::WrongPrompt)
            );
        }
        assert_eq!(
            state.accept_callback(start("owned-image-start", prompt.clone())),
            CallbackDisposition::AcceptedBinding
        );
        assert_eq!(
            state.accept_callback(ProviderCallback::final_event(
                "owned-image-final",
                12,
                claude,
                "launch-claude",
                "provider-session",
                "turn-1",
                &prompt,
                "IMAGE_CORRELATION_MARKER",
            )),
            CallbackDisposition::AcceptedCompleted
        );
        state
            .observe_status(claude, RuntimeStatus::Idle, 13)
            .unwrap();
        let settled = state.request(request).unwrap();
        assert_eq!(settled.phase, RequestPhase::Completed);
        assert!(settled.trusted_start_bound && state.turn_ended(settled));
        assert_eq!(
            settled.provider_session_id.as_deref(),
            Some("provider-session")
        );
        assert_eq!(settled.provider_turn_id.as_deref(), Some("turn-1"));
        assert!(settled.failure_reason.is_none());
        assert!(
            settled.reply().is_none(),
            "a native final is not an explicit Bus reply"
        );
        assert_eq!(settled.submitted_payload.as_deref(), Some(frozen.as_str()));
        assert_eq!(settled.prompt.text, typed);
        assert_eq!(state.next_queued_request(claude), Some(queued));
    }
}

#[test]
fn each_provider_keeps_literal_image_text_and_rejects_foreign_payloads() {
    for provider in [
        Provider::ClaudeCode,
        Provider::ClaudeDs,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        let mut state = BusState::default();
        let room = state.create_room("images").unwrap();
        let agent = state
            .create_agent(room, "worker", provider, "/repo".into(), None)
            .unwrap();
        state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some("launch".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let request = submit_text(
            &mut state,
            room,
            agent,
            "Literal image text\n/private/tmp/fixture.png",
        );
        let typed = state.render_request_payload(request).unwrap();
        submit_request(&mut state, request, "launch", 10);
        assert!(state
            .request(request)
            .unwrap()
            .matches_callback_payload(&typed));
        assert!(!state
            .request(request)
            .unwrap()
            .matches_callback_payload("Literal image text"));
        assert!(!state
            .request(request)
            .unwrap()
            .matches_callback_payload(&format!("{typed} extra")));
    }
}
