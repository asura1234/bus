//! Red-first regression suite for live incident 23176/23177:
//! AGY native paste+Enter confirmed, provider turn ran in transcript without a
//! fresh PreInvocation binding (steered after an empty PLANNER_RESPONSE step, or
//! standalone Stop with transcript proof), plus the neighbour where no input
//! ever reached the transcript.
use super::*;

/// Live fixture prompt text from Bus message 23176 / request 23177 to
/// `placeholder-reviewer` (agent 23126, session `9aa19792-6d55-462f-854b-60206318e07d`).
const MSG_23176_TEXT: &str = "Finding 1 已转给 placeholder-style 修复。作者会推新 head 并通知你；请只复审该 delta 与相关回归测试，确认修复不改变已验收的视觉/行为后给 Ready verdict，写入同一 temp/review-pr/ 路径并回 #824。若修复引入行为变化，要求重新真机验证。";
const SESSION_23176: &str = "9aa19792-6d55-462f-854b-60206318e07d";

fn agy_fixture_for_session(session: &str) -> Fixture {
    let mut fixture = Fixture::new(Provider::Agy);
    let mut state = fixture.worker.state.clone();
    let mut identity = state.agent(fixture.agent).unwrap().runtime_identity.clone();
    identity.session_id = Some(session.to_owned());
    state
        .set_agent_runtime_identity(fixture.agent, identity)
        .unwrap();
    fixture.worker.save(state).unwrap();
    fixture
}

fn write_transcript(fixture: &Fixture, session: &str, rows: &[Value]) -> std::path::PathBuf {
    let logs = fixture
        .dir
        .join(format!("agy-brain/{session}/.system_generated/logs"));
    std::fs::create_dir_all(&logs).unwrap();
    let transcript = logs.join("transcript_full.jsonl");
    let body = rows
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    std::fs::write(&transcript, body).unwrap();
    transcript
}

fn record_agy_stop(fixture: &Fixture, session: &str, transcript: &std::path::Path) {
    fixture.record(json!({
        "hookEventName": "Stop",
        "conversationId": session,
        "terminationReason": "NO_TOOL_CALL",
        "fullyIdle": true,
        "transcriptPath": transcript,
    }));
}

fn record_agy_pre_invocation(
    fixture: &Fixture,
    session: &str,
    step: u64,
    invocation_num: u64,
    prompt: &str,
    transcript: &std::path::Path,
) {
    fixture.record(json!({
        "hookEventName": "PreInvocation",
        "conversationId": session,
        "invocationNum": invocation_num,
        "initialNumSteps": step + 1,
        "lastUserInput": prompt,
        "transcriptPath": transcript,
    }));
}

#[test]
fn agy_steered_prompt_after_empty_planner_step_reconciles_from_transcript_23176() {
    let mut fixture = agy_fixture_for_session(SESSION_23176);

    // Lead request (mirrors req 23133 on turn 6).
    let lead = fixture.send("Review MR !1491 round 1", false);
    fixture.deliver();
    let lead_payload = fixture.typed().last().unwrap().0.clone();
    let transcript = write_transcript(
        &fixture,
        SESSION_23176,
        &[json!({
            "step_index": 6,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "content": format!("<USER_REQUEST>\n{lead_payload}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-11-20T14:27:00+08:00.\n</ADDITIONAL_METADATA>"),
        })],
    );
    record_agy_pre_invocation(&fixture, SESSION_23176, 6, 0, &lead_payload, &transcript);
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    assert!(fixture.request(lead).trusted_start_bound);
    assert_eq!(fixture.request(lead).provider_turn_id.as_deref(), Some("6"));

    // Steered follow-up (mirrors message 23176 / request 23177): paste+Enter confirmed into running turn.
    let steered = fixture.send(MSG_23176_TEXT, false);
    fixture.deliver();
    let steered_payload = fixture.typed().last().unwrap().0.clone();
    assert_eq!(fixture.request(steered).group, Some(lead));
    assert!(fixture.request(steered).steered);
    assert!(!fixture.request(steered).trusted_start_bound);

    // Exact live transcript shape from agent 23126:
    // step 123: empty PLANNER_RESPONSE (no tool_calls, no content) ends turn 6
    // step 124: steered 23176 USER_EXPLICIT/USER_INPUT ingested under invocationNum > 0
    // step 125: MODEL PLANNER_RESPONSE ("\n") finishes the steered input
    // Stop fires once for the whole session turn.
    write_transcript(
        &fixture,
        SESSION_23176,
        &[
            json!({
                "step_index": 6,
                "source": "USER_EXPLICIT",
                "type": "USER_INPUT",
                "status": "DONE",
                "content": format!("<USER_REQUEST>\n{lead_payload}\n</USER_REQUEST>"),
            }),
            json!({
                "step_index": 123,
                "source": "MODEL",
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "model": "gemini-4-neon-high",
                "output_tokens": 112,
            }),
            json!({
                "step_index": 124,
                "source": "USER_EXPLICIT",
                "type": "USER_INPUT",
                "status": "DONE",
                "content": format!("<USER_REQUEST>\n{steered_payload}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-11-20T14:41:42+08:00.\n</ADDITIONAL_METADATA>"),
            }),
            json!({
                "step_index": 125,
                "source": "MODEL",
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "model": "gemini-4-neon-high",
                "output_tokens": 250,
                "content": "\n",
            }),
        ],
    );
    record_agy_pre_invocation(
        &fixture,
        SESSION_23176,
        124,
        9,
        &steered_payload,
        &transcript,
    );
    record_agy_stop(&fixture, SESSION_23176, &transcript);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();

    let req = fixture.request(steered);
    assert_eq!(
        req.phase,
        RequestPhase::Completed,
        "23176 steered prompt must reconcile and settle Completed instead of Abandoned: {req:?}"
    );
    assert!(
        req.trusted_start_bound,
        "reconciled request must be start_bound: {req:?}"
    );
    assert_eq!(
        req.provider_session_id.as_deref(),
        Some(SESSION_23176),
        "reconciled request must carry session identity: {req:?}"
    );
    assert_eq!(
        req.provider_turn_id.as_deref(),
        Some("124"),
        "reconciled request must bind to its transcript turn step 124: {req:?}"
    );
    assert!(
        !req.uncertain_outcome,
        "reconciled request must not invent uncertainty: {req:?}"
    );
    assert_eq!(req.failure_reason, None, "{req:?}");
    assert_eq!(req.submission_error, None, "{req:?}");
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        None
    );
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .actionable_error,
        None
    );
}

#[test]
fn agy_confirmed_unbound_prompt_binds_and_settles_on_stop_without_prior_pre_invocation() {
    let mut fixture = agy_fixture_for_session(SESSION_23176);

    // Standalone AGY request: native paste+Enter confirmed (AgentPrompted -> Active),
    // but no PreInvocation(invocationNum == 0) bound it before Stop arrived.
    let req_id = fixture.send(MSG_23176_TEXT, false);
    fixture.deliver();
    let payload = fixture.typed().last().unwrap().0.clone();
    assert_eq!(fixture.request(req_id).phase, RequestPhase::Active);
    assert!(!fixture.request(req_id).trusted_start_bound);
    assert_eq!(fixture.request(req_id).provider_turn_id, None);

    let transcript = write_transcript(
        &fixture,
        SESSION_23176,
        &[
            json!({
                "step_index": 124,
                "source": "USER_EXPLICIT",
                "type": "USER_INPUT",
                "status": "DONE",
                "content": format!("<USER_REQUEST>\n{payload}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nThe current local time is: 2026-11-20T14:41:42+08:00.\n</ADDITIONAL_METADATA>"),
            }),
            json!({
                "step_index": 125,
                "source": "MODEL",
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "content": "Ready verdict confirmed for delta.",
            }),
        ],
    );
    record_agy_stop(&fixture, SESSION_23176, &transcript);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();

    let req = fixture.request(req_id);
    assert_eq!(
        req.phase,
        RequestPhase::Completed,
        "confirmed unbound request whose turn ran in transcript must settle Completed: {req:?}"
    );
    assert!(req.trusted_start_bound, "{req:?}");
    assert_eq!(
        req.provider_session_id.as_deref(),
        Some(SESSION_23176),
        "{req:?}"
    );
    assert_eq!(req.provider_turn_id.as_deref(), Some("124"), "{req:?}");
    assert!(!req.uncertain_outcome, "{req:?}");
    assert_eq!(
        fixture.reply(req_id).as_deref(),
        Some("Ready verdict confirmed for delta.")
    );
}

#[test]
fn agy_neighbour_ownership_failure_before_any_input_never_binds_another_turn() {
    let mut fixture = agy_fixture_for_session(SESSION_23176);

    // Lead turn 6 runs and completes cleanly.
    let lead = fixture.send("Review MR !1491 round 1", false);
    fixture.deliver();
    let lead_payload = fixture.typed().last().unwrap().0.clone();
    let transcript = write_transcript(
        &fixture,
        SESSION_23176,
        &[
            json!({
                "step_index": 6,
                "source": "USER_EXPLICIT",
                "type": "USER_INPUT",
                "status": "DONE",
                "content": format!("<USER_REQUEST>\n{lead_payload}\n</USER_REQUEST>"),
            }),
            json!({
                "step_index": 123,
                "source": "MODEL",
                "type": "PLANNER_RESPONSE",
                "status": "DONE",
                "content": "Round 1 blocked on Finding 1.",
            }),
        ],
    );
    record_agy_pre_invocation(&fixture, SESSION_23176, 6, 0, &lead_payload, &transcript);
    fixture.consume();
    fixture.status(RuntimeStatus::Working);

    // Steered follow-up never reached the provider transcript (ownership genuinely failed).
    let unreceived = fixture.send(MSG_23176_TEXT, false);
    fixture.deliver();
    assert!(!fixture.request(unreceived).trusted_start_bound);

    // Stop fires for turn 6 ONLY; transcript has no step for `unreceived`.
    record_agy_stop(&fixture, SESSION_23176, &transcript);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();

    // Lead completes with its own answer; unreceived follow-up must NOT bind to turn 6
    // and must NOT borrow lead's answer.
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Round 1 blocked on Finding 1.")
    );
    let missed = fixture.request(unreceived);
    assert!(
        !missed.trusted_start_bound,
        "an unreceived prompt absent from the transcript must never bind: {missed:?}"
    );
    assert_eq!(missed.provider_turn_id, None, "{missed:?}");
    assert_eq!(fixture.reply(unreceived), None, "{missed:?}");
    assert_eq!(missed.phase, RequestPhase::Abandoned, "{missed:?}");
}
