use super::*;
use serde_json::json;

fn candidate() -> Value {
    json!({"hookEventName":"stop", "sessionId":"synthetic-session",
        "promptId":"synthetic-turn", "reason":"end_turn",
        "lastAssistantMessage":"Synthetic answer"})
}

struct Transcript {
    root: std::path::PathBuf,
    path: std::path::PathBuf,
}
impl Transcript {
    fn new(contents: &[u8]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bus-grok-b-{}-{}",
            std::process::id(),
            crate::utils::time::now_ms()
        ));
        let dir = root.join("synthetic-session");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("updates.jsonl");
        std::fs::write(&path, contents).unwrap();
        Self { root, path }
    }
    fn value(&self) -> Value {
        let mut value = candidate();
        value["transcriptPath"] = json!(self.path);
        value
    }
}
impl Drop for Transcript {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn grok_unsettled_or_unproven_stop_is_pending() {
    for idle in [false, true] {
        assert_eq!(
            resolve_candidate(&candidate(), idle).unwrap(),
            CompletionResolution::Pending
        );
    }
}

#[test]
fn grok_completion_rejects_absent_or_wrong_identity() {
    for key in ["sessionId", "promptId"] {
        for bad in [Value::Null, json!(""), json!(1), json!(" ")] {
            let mut value = candidate();
            value[key] = bad;
            assert!(resolve_candidate(&value, true).is_err());
        }
    }
}

#[test]
fn grok_cancel_and_failure_end_matching_turn_without_answer() {
    let mut value = candidate();
    value["hookEventName"] = json!("stop_cancelled");
    value["reason"] = json!("user_interrupt");
    assert_eq!(
        resolve_candidate(&value, false).unwrap(),
        CompletionResolution::Cancelled {
            session: "synthetic-session".into(),
            turn: "synthetic-turn".into(),
            reason: "user_interrupt".into()
        }
    );
    value["hookEventName"] = json!("stop_failure");
    value["error"] = json!("server_error");
    assert!(matches!(resolve_candidate(&value, true).unwrap(),
        CompletionResolution::Cancelled { session, turn, .. }
        if session == "synthetic-session" && turn == "synthetic-turn"));
}

#[test]
fn grok_shutdown_child_and_unattributed_idle_do_not_resolve_turns() {
    let mut value = candidate();
    value["reason"] = json!("shutdown");
    assert_eq!(
        resolve_candidate(&value, true).unwrap(),
        CompletionResolution::Pending
    );
    value = candidate();
    value["subagentType"] = json!("explore");
    assert_eq!(
        resolve_candidate(&value, true).unwrap(),
        CompletionResolution::Pending
    );
    value = json!({"hookEventName":"notification", "sessionId":"synthetic-session",
        "notificationType":"idle_prompt"});
    assert_eq!(
        resolve_candidate(&value, true).unwrap(),
        CompletionResolution::Pending
    );
}

#[test]
fn grok_offline_hook_execution_success_is_not_turn_completion() {
    let capture = include_str!("fixtures/updates.jsonl");
    let session = "01a162bf-5091-7c42-9b5c-47930d378acd";
    let transcript = Transcript::new(capture.as_bytes());
    let directory = transcript.root.join(session);
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("updates.jsonl");
    std::fs::rename(&transcript.path, &path).unwrap();
    let mut value = candidate();
    value["sessionId"] = json!(session);
    value["transcriptPath"] = json!(path);
    assert_eq!(
        resolve_candidate(&value, true).unwrap(),
        CompletionResolution::Pending
    );
}

#[test]
fn grok_missing_delayed_partial_and_unknown_transcripts_stay_pending() {
    let transcript = Transcript::new(b"");
    assert_eq!(
        resolve_candidate(&transcript.value(), true).unwrap(),
        CompletionResolution::Pending
    );
    for contents in [
        b"{".as_slice(),
        b"{\"method\":\"_x.ai/session/update\",\"params\":{\"sessionId\":\"synthetic-session\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"Synthetic answer\"}}}}\n",
        b"{\"type\":\"turn_complete\",\"sessionId\":\"synthetic-session\",\"promptId\":\"synthetic-turn\",\"text\":\"invented final\"}\n"
    ] {
        std::fs::write(&transcript.path, contents).unwrap();
        assert_eq!(resolve_candidate(&transcript.value(), true).unwrap(), CompletionResolution::Pending);
    }
    std::fs::remove_file(&transcript.path).unwrap();
    assert_eq!(
        resolve_candidate(&transcript.value(), true).unwrap(),
        CompletionResolution::Pending
    );
}

#[test]
fn grok_transcript_path_must_belong_to_callback_session() {
    let transcript = Transcript::new(b"");
    let mut value = transcript.value();
    value["sessionId"] = json!("another-session");
    assert!(resolve_candidate(&value, true).is_err());
    value = candidate();
    value["transcriptPath"] = json!("relative/updates.jsonl");
    assert!(resolve_candidate(&value, true).is_err());
    value["transcriptPath"] = json!(transcript.root);
    assert!(resolve_candidate(&value, true).is_err());
}

#[test]
fn grok_clipped_and_long_answers_never_publish_without_final_proof() {
    let transcript = Transcript::new(b"");
    for text in [
        "x".repeat(32768),
        "😀".repeat(32768),
        format!("{}… [+42 chars]", "x".repeat(32768)),
    ] {
        let mut value = transcript.value();
        value["lastAssistantMessage"] = json!(text);
        assert_eq!(
            resolve_candidate(&value, true).unwrap(),
            CompletionResolution::Pending
        );
    }
}

#[test]
fn grok_oversized_or_torn_transcripts_never_publish_prefix() {
    let transcript = Transcript::new(&vec![b'x'; 2 * 1024 * 1024 + 1]);
    assert_eq!(
        resolve_candidate(&transcript.value(), true).unwrap(),
        CompletionResolution::Pending
    );
    std::fs::write(&transcript.path, b"{}\n{\"params\":").unwrap();
    assert_eq!(
        resolve_candidate(&transcript.value(), true).unwrap(),
        CompletionResolution::Pending
    );
}

#[cfg(unix)]
#[test]
fn grok_transcript_symlink_is_not_owned_evidence() {
    let transcript = Transcript::new(b"");
    let other = transcript.root.join("other.jsonl");
    std::fs::write(&other, b"").unwrap();
    std::fs::remove_file(&transcript.path).unwrap();
    std::os::unix::fs::symlink(&other, &transcript.path).unwrap();
    assert!(resolve_candidate(&transcript.value(), true).is_err());
}

#[test]
fn grok_reader_preserves_long_unicode_reply_and_discards_only_cut_prefix() {
    let text = "Synthetic long answer 😀\n".repeat(3000);
    let row = json!({"method":"_x.ai/session/update", "params": {
        "sessionId":"synthetic-session", "update":{"sessionUpdate":"agent_message_chunk",
        "content":{"type":"text", "text":text}}}});
    let line = format!("{row}\n");
    let transcript = Transcript::new(line.as_bytes());
    let read = read_transcript(&transcript.path, "synthetic-session")
        .unwrap()
        .unwrap();
    let recovered: Value = serde_json::from_str(read.trim_end()).unwrap();
    assert_eq!(
        recovered
            .pointer("/params/update/content/text")
            .unwrap()
            .as_str(),
        Some(text.as_str())
    );
    // The read window starts inside an old row. Never parse its cut fragment.
    let mut bytes = vec![b'x'; 2 * 1024 * 1024];
    bytes.push(b'\n');
    bytes.extend_from_slice(line.as_bytes());
    std::fs::write(&transcript.path, bytes).unwrap();
    assert_eq!(
        read_transcript(&transcript.path, "synthetic-session")
            .unwrap()
            .unwrap(),
        line
    );
    assert_eq!(
        resolve_candidate(&transcript.value(), true).unwrap(),
        CompletionResolution::Pending
    );
}

#[test]
fn grok_candidate_then_later_failure_or_cancel_keeps_original_turn_identity() {
    let first = candidate();
    assert_eq!(
        resolve_candidate(&first, true).unwrap(),
        CompletionResolution::Pending
    );
    for name in ["stop_failure", "stop_cancelled"] {
        let mut late = first.clone();
        late["hookEventName"] = json!(name);
        late["timestamp"] = json!("2099-01-01T00:00:00Z");
        late["reason"] = json!("user_interrupt");
        assert!(matches!(resolve_candidate(&late, true).unwrap(),
            CompletionResolution::Cancelled { session, turn, .. }
            if session == "synthetic-session" && turn == "synthetic-turn"));
    }
}

// Bus-defined attestations below are SYNTHETIC internal values, never Grok JSON.
fn attestation(text: &str) -> CompletionAttestation {
    CompletionAttestation {
        session: "synthetic-session".into(),
        turn: "synthetic-turn".into(),
        text: text.into(),
    }
}

#[test]
fn grok_attestation_is_required_even_when_idle() {
    for idle in [false, true] {
        assert_eq!(
            resolve_attested_completion("synthetic-session", "synthetic-turn", idle, None).unwrap(),
            CompletionResolution::Pending
        );
    }
    let proof = attestation("Synthetic committed reply");
    assert_eq!(
        resolve_attested_completion("synthetic-session", "synthetic-turn", false, Some(&proof))
            .unwrap(),
        CompletionResolution::Pending
    );
}

#[test]
fn grok_matching_idle_attestation_preserves_full_multiline_unicode_final() {
    let text = format!(
        "  Synthetic committed reply 😀
{}

",
        "full body 🦀
"
        .repeat(4000)
    );
    let proof = attestation(&text);
    assert_eq!(
        resolve_attested_completion("synthetic-session", "synthetic-turn", true, Some(&proof))
            .unwrap(),
        CompletionResolution::Final {
            session: "synthetic-session".into(),
            turn: "synthetic-turn".into(),
            text,
        }
    );
}

#[test]
fn grok_explicit_empty_final_is_distinct_from_missing_text_evidence() {
    let proof = attestation("");
    assert_eq!(
        resolve_attested_completion("synthetic-session", "synthetic-turn", true, Some(&proof))
            .unwrap(),
        CompletionResolution::Final {
            session: "synthetic-session".into(),
            turn: "synthetic-turn".into(),
            text: String::new(),
        }
    );
    let mut value = candidate();
    value
        .as_object_mut()
        .unwrap()
        .remove("lastAssistantMessage");
    assert_eq!(
        resolve_candidate(&value, true).unwrap(),
        CompletionResolution::Pending
    );
}

#[test]
fn grok_attestation_identity_mismatch_and_empty_identity_always_fail() {
    let proof = attestation("Synthetic reply");
    for idle in [false, true] {
        for (session, turn) in [
            ("", "synthetic-turn"),
            ("synthetic-session", ""),
            (" ", "synthetic-turn"),
            ("synthetic-session", " "),
            ("other-session", "synthetic-turn"),
            ("synthetic-session", "newer-turn"),
        ] {
            assert!(resolve_attested_completion(session, turn, idle, Some(&proof)).is_err());
        }
        for (session, turn) in [("", "synthetic-turn"), ("synthetic-session", "")] {
            assert!(resolve_attested_completion(session, turn, idle, None).is_err());
        }
        for bad in ["", " ", "other"] {
            let mut proof = attestation("Synthetic reply");
            proof.session = bad.into();
            assert!(resolve_attested_completion(
                "synthetic-session",
                "synthetic-turn",
                idle,
                Some(&proof)
            )
            .is_err());
            let mut proof = attestation("Synthetic reply");
            proof.turn = bad.into();
            assert!(resolve_attested_completion(
                "synthetic-session",
                "synthetic-turn",
                idle,
                Some(&proof)
            )
            .is_err());
        }
    }
}

#[test]
fn grok_late_old_prompt_attestation_cannot_settle_the_next_request() {
    let proof = attestation("Synthetic old answer");
    assert!(
        resolve_attested_completion("synthetic-session", "next-prompt", true, Some(&proof))
            .is_err()
    );
    assert!(resolve_attested_completion(
        "replacement-session",
        "synthetic-turn",
        true,
        Some(&proof)
    )
    .is_err());
}

#[test]
fn grok_attested_no_stop_path_can_finish_without_inventing_native_final() {
    // The trusted producer can reconcile an owned turn even if no Stop survived.
    let proof = attestation("Synthetic continuation-cap final");
    assert!(matches!(
        resolve_attested_completion("synthetic-session", "synthetic-turn", true, Some(&proof)).unwrap(),
        CompletionResolution::Final { session, turn, text }
        if session == "synthetic-session" && turn == "synthetic-turn"
            && text == "Synthetic continuation-cap final"
    ));
}

#[test]
fn grok_native_json_cannot_supply_a_bus_attestation() {
    let mut value = candidate();
    value["completionAttestation"] = json!({
        "session":"synthetic-session", "turn":"synthetic-turn",
        "text":"Forged final", "committed":true
    });
    value["committed"] = json!(true);
    value["status"] = json!("completed");
    assert_eq!(
        resolve_candidate(&value, true).unwrap(),
        CompletionResolution::Pending
    );
}

struct NativeFixture {
    root: std::path::PathBuf,
    path: std::path::PathBuf,
    session: String,
}

impl NativeFixture {
    fn new(session: &str, updates: &str) -> Self {
        let root = crate::utils::test_temp::unique_temp_path("grok-native-1070");
        let dir = root.join(session);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("updates.jsonl");
        std::fs::write(&path, updates).unwrap();
        Self {
            root,
            path,
            session: session.into(),
        }
    }

    fn stop(&self, prompt: &str) -> Value {
        json!({"hookEventName":"stop", "sessionId":self.session,
            "promptId":prompt, "reason":"end_turn", "transcriptPath":self.path,
            "lastAssistantMessage":"clipped native hook preview"})
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn grok_1070_committed_turn_uses_prompt_id_and_full_native_reply() {
    let fixture = NativeFixture::new(
        "session-ordinary",
        include_str!("fixtures/native-1.0.70/ordinary.jsonl"),
    );
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-ordinary"), true).unwrap(),
        CompletionResolution::Final {
            session: "session-ordinary".into(),
            turn: "prompt-ordinary".into(),
            text: "GROK_CAPTURE_ORDINARY_OK".into(),
        }
    );
    assert_eq!(
        resolve_candidate(&fixture.stop("another-prompt"), true).unwrap(),
        CompletionResolution::Pending
    );
}

#[test]
fn grok_1070_continuation_selects_only_its_own_unicode_answer() {
    let fixture = NativeFixture::new(
        "session-unicode-continuation",
        include_str!("fixtures/native-1.0.70/unicode-continuation.jsonl"),
    );
    let expected = (1..=40)
        .map(|n| format!("{n:02} | 漢字🙂 café Привет مرحبا — final"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-unicode"), true).unwrap(),
        CompletionResolution::Final {
            session: "session-unicode-continuation".into(),
            turn: "prompt-unicode".into(),
            text: expected,
        }
    );
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-continuation"), true).unwrap(),
        CompletionResolution::Final {
            session: "session-unicode-continuation".into(),
            turn: "prompt-continuation".into(),
            text: "CONTINUATION_2_OK 🙂".into(),
        }
    );
}

#[test]
fn grok_1070_long_unicode_reply_comes_from_the_native_record() {
    let fixture = NativeFixture::new(
        "session-long-unicode",
        include_str!("fixtures/native-1.0.70/long-unicode-180.jsonl"),
    );
    let expected = (1..=180)
        .map(|n| format!("{n:03} | 漢字🙂 café Привет مرحبا — complete"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(expected.len() > 7_000);
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-long-unicode"), true).unwrap(),
        CompletionResolution::Final {
            session: "session-long-unicode".into(),
            turn: "prompt-long-unicode".into(),
            text: expected,
        }
    );
}

#[test]
fn grok_1070_clipped_stop_preview_cannot_truncate_the_committed_answer() {
    use sha2::{Digest, Sha256};

    let fixture = NativeFixture::new(
        "session-clipped",
        include_str!("fixtures/native-1.0.70/clipped-answer.jsonl"),
    );
    let hooks: Vec<Value> = serde_json::from_str(include_str!(
        "fixtures/native-1.0.70/clipped-answer-hooks.json"
    ))
    .unwrap();
    let mut stop = hooks[1].clone();
    let preview = stop["lastAssistantMessage"].as_str().unwrap().to_owned();
    assert!(preview.contains("… [+4071 chars]"));
    stop["transcriptPath"] = json!(fixture.path);
    let CompletionResolution::Final {
        session,
        turn,
        text,
    } = resolve_candidate(&stop, true).unwrap()
    else {
        panic!("clipped hook preview must not obscure a committed native final");
    };
    assert_eq!(session, "session-clipped");
    assert_eq!(turn, "prompt-clipped");
    assert_eq!(text.lines().count(), 120);
    assert!(text.len() > preview.len());
    assert_eq!(
        format!("{:x}", Sha256::digest(text.as_bytes())),
        "a989c9a33d6740ae7733ebfed2b88b1bd28f8439856a686a31ac6d848833f0e5"
    );
}

#[test]
fn grok_1070_tool_preamble_is_not_the_committed_final() {
    let fixture = NativeFixture::new(
        "session-tool-final",
        include_str!("fixtures/native-1.0.70/tool-final.jsonl"),
    );
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-tool"), true).unwrap(),
        CompletionResolution::Final {
            session: "session-tool-final".into(),
            turn: "prompt-tool".into(),
            text: "<SCRATCH_PROJECT>".into(),
        }
    );
}

#[test]
fn grok_1070_blocked_stop_uses_the_post_gate_continuation() {
    let fixture = NativeFixture::new(
        "session-gate-continuation",
        include_str!("fixtures/native-1.0.70/gate-continuation.jsonl"),
    );
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-gate-continuation"), true).unwrap(),
        CompletionResolution::Final {
            session: "session-gate-continuation".into(),
            turn: "prompt-gate-continuation".into(),
            text: "GATE_CONTINUED_OK".into(),
        }
    );
}

#[test]
fn grok_1070_interrupted_stop_gate_has_no_committed_final() {
    let fixture = NativeFixture::new(
        "session-gate-cancel",
        include_str!("fixtures/native-1.0.70/gate-cancel.jsonl"),
    );
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-gate-cancel"), true).unwrap(),
        CompletionResolution::Pending
    );
}

#[test]
fn grok_1070_no_approval_tool_turn_still_needs_commit() {
    let fixture = NativeFixture::new(
        "session-always-approve",
        include_str!("fixtures/native-1.0.70/always-approve.jsonl"),
    );
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-always-approve"), true).unwrap(),
        CompletionResolution::Final {
            session: "session-always-approve".into(),
            turn: "prompt-always-approve".into(),
            text: "Yolo tool completed.".into(),
        }
    );
}

#[test]
fn grok_1070_cancel_and_max_turns_never_publish_a_reply() {
    for (session, prompt, updates) in [
        (
            "session-cancel-early",
            "cancelled-prompt",
            include_str!("fixtures/native-1.0.70/cancel-early.jsonl"),
        ),
        (
            "session-max-turns-failure",
            "prompt-cap",
            include_str!("fixtures/native-1.0.70/max-turns-failure.jsonl"),
        ),
    ] {
        let fixture = NativeFixture::new(session, updates);
        assert_eq!(
            resolve_candidate(&fixture.stop(prompt), true).unwrap(),
            CompletionResolution::Pending
        );
    }
}

#[test]
fn grok_1070_torn_commit_and_wrong_session_fail_closed() {
    let fixture = NativeFixture::new(
        "session-ordinary",
        include_str!("fixtures/native-1.0.70/ordinary.jsonl"),
    );
    let mut bytes = std::fs::read(&fixture.path).unwrap();
    bytes.pop();
    std::fs::write(&fixture.path, bytes).unwrap();
    assert_eq!(
        resolve_candidate(&fixture.stop("prompt-ordinary"), true).unwrap(),
        CompletionResolution::Pending
    );
    std::fs::write(
        &fixture.path,
        include_str!("fixtures/native-1.0.70/ordinary.jsonl"),
    )
    .unwrap();
    let mut wrong = fixture.stop("prompt-ordinary");
    wrong["sessionId"] = json!("other-session");
    assert!(resolve_candidate(&wrong, true).is_err());
}
