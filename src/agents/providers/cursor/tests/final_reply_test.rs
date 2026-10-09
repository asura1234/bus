use super::*;

const TURN: &str = concat!(
    "{\"role\":\"user\",\"message\":{\"content\":[]}}\n",
    "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Question first.\"},{\"type\":\"tool_use\",\"name\":\"AskQuestion\"}]}}\n",
    "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"final\"},{\"type\":\"text\",\"text\":\" answer\"}]}}\n",
    "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
);

#[test]
fn cursor_final_reply_excludes_question_commentary_and_keeps_message_blocks() {
    for hook in ["Question first.final answer", "final answer"] {
        assert_eq!(
            matching_completed_message(TURN, hook).as_deref(),
            Some("final answer")
        );
    }
}

/// 消息 1422：后续问题的说明块比钩子多一段后缀，且 jsonl 没有 turn_ended。
#[test]
fn cursor_final_reply_ignores_commentary_suffix_the_hook_omits() {
    for omitted in ["\\n\\n[REDACTED]", "\\n\\n<dropped>"] {
        let transcript = format!(
            "{{\"role\":\"user\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"research\"}}]}}}}\n\
             {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"I'll check.\"}},{{\"type\":\"tool_use\",\"name\":\"Shell\"}}]}}}}\n\
             {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"First answer.\"}}]}}}}\n\
             {{\"role\":\"user\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"also search the web\"}}]}}}}\n\
             {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"I'll search the docs.{omitted}\"}},{{\"type\":\"tool_use\",\"name\":\"CallDynamicTool\"}}]}}}}\n\
             {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"name\":\"Grep\"}}]}}}}\n\
             {{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"Second answer.\"}}]}}}}\n"
        );
        assert_eq!(
            matching_completed_message(&transcript, "I'll search the docs.Second answer.")
                .as_deref(),
            Some("Second answer."),
            "{omitted}"
        );
        assert_eq!(
            matching_completed_message(&transcript, "I'll check.First answer.").as_deref(),
            Some("First answer."),
            "{omitted}"
        );
    }
    // 终稿里的同样字样属于答案，钩子带上了就必须原样匹配。
    let kept = concat!(
        "{\"role\":\"user\",\"message\":{\"content\":[]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"see [REDACTED]\"}]}}\n",
    );
    assert_eq!(
        matching_completed_message(kept, "see [REDACTED]").as_deref(),
        Some("see [REDACTED]")
    );
}

#[test]
fn cursor_final_reply_waits_for_completed_matching_turn() {
    assert!(matching_completed_message(TURN, "unrelated response").is_none());
    for incomplete in [
        TURN.replace("\"status\":\"success\"", "\"status\":\"error\""),
        TURN.lines().take(2).collect::<Vec<_>>().join("\n"),
        format!("{TURN}{{\"role\":\"assistant\""),
    ] {
        assert!(matching_completed_message(&incomplete, "Question first.final answer").is_none());
    }
    // 终稿已经在文件里、只是没有 turn_ended 时也要结算。
    assert_eq!(
        matching_completed_message(
            &TURN.lines().take(3).collect::<Vec<_>>().join("\n"),
            "Question first.final answer"
        )
        .as_deref(),
        Some("final answer")
    );
}

#[test]
fn cursor_final_reply_matches_its_completed_turn_after_a_newer_turn_starts() {
    let newer = "{\"role\":\"user\",\"message\":{\"content\":[]}}\n";
    assert_eq!(
        matching_completed_message(&format!("{TURN}{newer}"), "Question first.final answer")
            .as_deref(),
        Some("final answer")
    );
    let different = TURN.replace("final", "newer");
    assert_eq!(
        matching_completed_message(&format!("{TURN}{different}"), "Question first.final answer")
            .as_deref(),
        Some("final answer")
    );
}

#[test]
fn cursor_final_reply_ignores_injected_user_records_within_completed_turn() {
    let transcript = concat!(
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"review\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Checking.\"},{\"type\":\"tool_use\",\"name\":\"Read\"}]}}\n",
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<available_subagent_types>...</available_subagent_types>\"}]}}\n",
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"review\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Ready\"}]}}\n",
        "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
    );

    assert_eq!(
        matching_completed_message(transcript, "Checking.Ready").as_deref(),
        Some("Ready")
    );
}

#[test]
fn cursor_final_reply_recovers_prior_generation_after_transcript_advances() {
    let transcript = concat!(
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"older\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Older answer\"}]}}\n",
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"review\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Checking.\"},{\"type\":\"tool_use\",\"name\":\"Read\"}]}}\n",
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<available_subagent_types>...</available_subagent_types>\"}]}}\n",
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"review\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Ready\"}]}}\n",
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"newer\"}]}}\n",
        "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Newer answer\"}]}}\n",
        "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
    );

    assert_eq!(
        matching_completed_message(transcript, "Checking.Ready").as_deref(),
        Some("Ready")
    );
}

#[test]
fn cursor_final_reply_allows_repeated_identical_answers_but_not_conflicting_splits() {
    let ok = TURN
        .lines()
        .filter(|line| !line.contains("Question first."))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\"final\"", "\"O\"")
        .replace("\" answer\"", "\"K\"");
    assert_eq!(
        matching_completed_message(&format!("{ok}\n{ok}\n"), "OK").as_deref(),
        Some("OK")
    );
    assert_eq!(
        matching_completed_message(&format!("{TURN}{TURN}"), "Question first.final answer")
            .as_deref(),
        Some("final answer")
    );
    // Same aggregate text, but different commentary/final boundaries.
    let conflicting = TURN
        .replace("Question first.", "Question ")
        .replace("\"final\"", "\"first.final\"");
    assert!(matching_completed_message(
        &format!("{TURN}{conflicting}"),
        "Question first.final answer"
    )
    .is_none());
}

fn hook_value(text: &str, transcript: &str) -> Value {
    let path = std::env::temp_dir().join(format!(
        "bus-cursor-settle-{}-{}.jsonl",
        std::process::id(),
        text.len() + transcript.len()
    ));
    std::fs::write(&path, transcript).unwrap();
    serde_json::json!({"text": text, "transcript_path": path})
}

#[test]
fn idle_completed_stop_uses_the_hook_text_when_the_transcript_does_not_match() {
    // 消息 1422 的钩子形状：说明和终稿粘在一起，转录对不上。
    let hook = "I'll search.The published docs still have no launch-time system prompt.";
    let value = hook_value(
        hook,
        concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"addendum\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"unrelated\"}]}}\n",
        ),
    );
    assert!(settle_text(&value, false).is_err());
    assert_eq!(settle_text(&value, true).as_deref(), Ok(hook));
}

#[test]
fn a_matching_transcript_still_beats_the_raw_hook_text() {
    let value = hook_value(
        "I'll search.The published docs still have no launch-time system prompt.",
        concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"addendum\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I'll search.\\n\\n[REDACTED]\"},{\"type\":\"tool_use\",\"name\":\"WebSearch\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"The published docs still have no launch-time system prompt.\"}]}}\n",
        ),
    );
    assert_eq!(
        settle_text(&value, true).as_deref(),
        Ok("The published docs still have no launch-time system prompt.")
    );
}

#[test]
fn background_task_notices_are_not_room_prompts() {
    let wake = "<timestamp>Wednesday, Oct 7, 2026, 8:30 PM (UTC+8)</timestamp>  <user_query>Briefly inform the user about the task result and perform any follow-up actions (if needed). If there's no follow-ups needed, don't explicitly say that.</user_query>";
    assert!(is_background_task_notice(wake));
    assert!(is_background_task_notice(
        "Finished Resume session after changing the ancestor rule"
    ));
    assert!(!is_background_task_notice(
        "Bug fix task. The notice was: Briefly inform the user about the task result. Then fix the matcher."
    ));
    assert!(!is_background_task_notice(
        "Finished the design.\n\nHere is the rest of the work."
    ));
}

#[test]
fn background_task_notice_detection_preserves_human_text_before_quoted_timestamp() {
    let prompt = "Explain why this logged prompt was ignored: <timestamp>Wednesday</timestamp> <user_query>Briefly inform the user about the task result</user_query>";
    assert!(!is_background_task_notice(prompt));
}

#[test]
fn human_request_quoting_a_cursor_task_notice_keeps_its_final_reply() {
    use crate::messaging::model::{
        AgentRuntimeIdentity, BusState, CallbackDisposition, CallbackEventKind, Provider,
        ProviderCallback, RequestPhase, RuntimeStatus, SubmissionOutcome,
    };

    let prompt = "Explain why this logged prompt was ignored: <timestamp>Wednesday</timestamp> <user_query>Briefly inform the user about the task result</user_query>";
    let mut state = BusState::new();
    let room = state.create_room("debug").unwrap();
    let agent = state
        .create_agent(room, "reader", Provider::Cursor, "/repo".into(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch-cursor".into()),
                terminal_id: None,
                pane_id: None,
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.set_draft_text(room, prompt).unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, 10).unwrap()[0];
    state
        .begin_submission(request, "launch-cursor", 10)
        .unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("generation".into()),
            },
        )
        .unwrap();
    let started = state.accept_callback(ProviderCallback {
        callback_id: "human-start".into(),
        sequence: 11,
        occurred_at_ms: 11,
        agent_id: agent,
        launch_id: "launch-cursor".into(),
        provider_session_id: Some("session".into()),
        provider_turn_id: Some("generation".into()),
        provider_prompt_id: None,
        prompt_payload: Some(prompt.into()),
        kind: CallbackEventKind::PromptStarted,
    });
    let finished = state.accept_callback(ProviderCallback::final_event(
        "human-final",
        12,
        agent,
        "launch-cursor",
        "session",
        "generation",
        prompt,
        "explained",
    ));
    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .unwrap();
    let reply = state
        .room(room)
        .unwrap()
        .latest_replies
        .get(&agent)
        .map(|reply| reply.text.as_str());
    assert_eq!(
        (
            started,
            finished,
            state.request(request).unwrap().phase,
            reply
        ),
        (
            CallbackDisposition::AcceptedBinding,
            CallbackDisposition::AcceptedPendingSettlement,
            RequestPhase::Completed,
            Some("explained"),
        )
    );
}
