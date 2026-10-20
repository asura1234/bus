use super::turn_settlement_tests::{claude_startup, claude_stop};
use super::*;

#[test]
fn background_stops_keep_their_reply_and_release_at_the_first_hold_deadline() {
    for tasks in [
        json!([]),
        json!([{"id":"shell-1","type":"shell","status":"running","command":"sleep 3600"}]),
        json!([{"id":"agent-1","type":"local_agent","status":"running"}]),
    ] {
        let mut f = Fixture::new(Provider::ClaudeCode);
        f.record(claude_startup());
        f.consume();
        let lead = running(&mut f, "report progress", "turn-1");
        let mut stop = claude_stop("turn-1", "First report", &[]);
        stop["background_tasks"] = tasks.clone();
        stop["session_crons"] = json!([{"cron_id":"cron-1","status":"running"}]);
        f.record(stop.clone());
        f.consume();
        f.status(RuntimeStatus::Idle);
        let first_hold = f
            .request(lead)
            .shell_hold
            .as_ref()
            .expect("Stop text must be retained even when the parser reports background_pending")
            .since_ms;
        let next = f.send("next queued task", true);
        f.now += 5 * 60_000;
        f.started(
            "turn-2",
            "<task-notification>shell completed</task-notification>",
        );
        f.consume();
        f.status(RuntimeStatus::Working);
        stop["prompt_id"] = json!("turn-2");
        stop["last_assistant_message"] = json!("Latest report");
        f.record(stop);
        f.consume();
        f.status(RuntimeStatus::Idle);
        assert_eq!(f.request(lead).phase, RequestPhase::Active);
        assert_eq!(
            f.request(lead).shell_hold.as_ref().unwrap().since_ms,
            first_hold
        );
        let stored = f.worker.store.load().unwrap().unwrap();
        assert_eq!(
            stored.request(lead).unwrap().shell_hold,
            f.request(lead).shell_hold
        );
        f.now = first_hold + 15 * 60_000;
        f.status(RuntimeStatus::Idle);
        f.consume();
        assert_eq!(f.request(lead).phase, RequestPhase::Completed, "{tasks}");
        assert_eq!(f.reply(lead).as_deref(), Some("First report"));
        f.worker.expire_queued_requests(f.now).unwrap();
        assert_eq!(f.request(next).phase, RequestPhase::Queued);
        f.deliver();
        assert_eq!(f.request(next).phase, RequestPhase::Active);
    }
}

#[test]
fn background_stop_without_text_does_not_fabricate_a_reply() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.record(claude_startup());
    f.consume();
    let lead = running(&mut f, "report progress", "turn-1");
    f.record(
        json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
        "background_tasks":[],"session_crons":[{"cron_id":"cron-1"}]}),
    );
    f.consume();
    f.now += 15 * 60_000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert!(f.request(lead).awaiting_background);
    assert!(f.request(lead).shell_hold.is_none());
    assert!(f.reply(lead).is_none());
}

#[test]
fn background_stop_diagnostics_describe_the_hold_without_reply_or_task_payloads() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let mut f = Fixture::new(Provider::ClaudeCode);
        let lead = running(&mut f, "PRIVATE_PROMPT", "turn-1");
        f.record(
            json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
            "last_assistant_message":"PRIVATE_REPLY",
            "background_tasks":[{"type":"local_agent","description":"PRIVATE_TASK"}],
            "session_crons":[{"cron_id":"PRIVATE_CRON"}]}),
        );
        f.consume();
        assert!(f.request(lead).shell_hold.is_some());
    });
    let log = capture
        .text()
        .lines()
        .filter(|line| line.contains("bus.callback.background_pending"))
        .collect::<Vec<_>>()
        .join("\n");
    for field in [
        "captured_reply_bytes=13",
        "session_cron_count=1",
        "non_shell_task_count=1",
    ] {
        assert!(log.contains(field), "{log}");
    }
    for private in [
        "PRIVATE_PROMPT",
        "PRIVATE_REPLY",
        "PRIVATE_TASK",
        "PRIVATE_CRON",
    ] {
        assert!(!log.contains(private), "{log}");
    }
}

fn scheduled_start(f: &Fixture, turn: &str, prompt: &str) {
    let transcript = f.dir.join("scheduled.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type":"user", "sessionId":"session", "promptId":turn,
                "isMeta":true, "promptSource":"system", "turnOrigin":"scheduled",
                "scheduledTaskId":"cron-1", "message":{"role":"user","content":prompt}
            })
        ),
    )
    .unwrap();
    f.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "prompt_id":turn,"prompt":prompt,"transcript_path":transcript}),
    );
}

fn cron_stop(turn: &str, text: &str) -> Value {
    let mut stop = claude_stop(turn, text, &["long-shell"]);
    stop["session_crons"] = json!([{"id":"cron-1","recurring":true}]);
    stop
}

#[test]
fn scheduled_cron_turn_cannot_replace_the_answer_to_a_held_bus_request() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "report progress", "turn-1");
    f.record(cron_stop("turn-1", "Requested answer"));
    f.consume();
    let since = f.request(lead).shell_hold.as_ref().unwrap().since_ms;
    scheduled_start(&f, "cron-turn", "Scheduled self reminder");
    f.consume();
    assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("turn-1"));
    f.record(cron_stop("cron-turn", "CRON_TICK"));
    f.consume();
    assert_eq!(
        f.request(lead).shell_hold.as_ref().unwrap().interim.text,
        "Requested answer"
    );
    f.now = since + 15 * 60_000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Requested answer"));
}

#[test]
fn rejected_background_stop_cannot_poison_the_retained_reply_or_its_deadline() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "report progress", "turn-1");
    f.record(cron_stop("turn-1", "Requested answer"));
    f.consume();
    let held = f.request(lead).shell_hold.clone();
    f.record(cron_stop("foreign-turn", "UNRELATED"));
    f.consume();
    assert_eq!(f.request(lead).shell_hold, held);
}

#[test]
fn scheduled_transcript_metadata_does_not_discard_input_bus_actually_submitted() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "report progress", "turn-1");
    f.record(cron_stop("turn-1", "First answer"));
    f.consume();
    f.status(RuntimeStatus::Idle);
    let next = f.send("Scheduled self reminder", false);
    f.deliver();
    scheduled_start(&f, "turn-2", "Scheduled self reminder");
    f.consume();
    f.stopped("turn-2", "Explicitly requested answer");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("First answer"));
    assert_eq!(
        f.reply(next).as_deref(),
        Some("Explicitly requested answer")
    );
}

#[test]
fn a_textless_background_wake_stop_still_releases_the_captured_reply_at_the_cap() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.record(claude_startup());
    f.consume();
    let lead = running(&mut f, "watch the deploy", "turn-1");
    f.record(cron_stop("turn-1", "Captured answer"));
    f.consume();
    f.status(RuntimeStatus::Idle);
    let since = f.request(lead).shell_hold.as_ref().unwrap().since_ms;
    f.started(
        "turn-2",
        "<task-notification>shell completed</task-notification>",
    );
    f.consume();
    f.status(RuntimeStatus::Working);
    f.record(cron_stop("turn-2", ""));
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now = since + 60 * 60_000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(
        f.request(lead).phase,
        RequestPhase::Completed,
        "a held request must not wait forever after a textless wake Stop"
    );
    assert_eq!(f.reply(lead).as_deref(), Some("Captured answer"));
}

#[test]
fn a_textless_shell_wake_stop_does_not_blank_the_captured_reply() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.record(claude_startup());
    f.consume();
    let lead = running(&mut f, "run the build", "turn-1");
    f.record(claude_stop("turn-1", "Captured answer", &["long-shell"]));
    f.consume();
    f.status(RuntimeStatus::Idle);
    let since = f.request(lead).shell_hold.as_ref().unwrap().since_ms;
    f.started(
        "turn-2",
        "<task-notification><task-id>other-shell</task-id></task-notification>",
    );
    f.consume();
    f.status(RuntimeStatus::Working);
    f.record(claude_stop("turn-2", "", &["long-shell"]));
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now = since + 60 * 60_000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Completed);
    assert_eq!(f.reply(lead).as_deref(), Some("Captured answer"));
}

#[test]
fn a_scheduled_turn_that_finishes_without_background_work_does_not_answer_the_held_request() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.record(claude_startup());
    f.consume();
    let lead = running(&mut f, "report progress", "turn-1");
    f.record(cron_stop("turn-1", "Requested answer"));
    f.consume();
    f.status(RuntimeStatus::Idle);
    let since = f.request(lead).shell_hold.as_ref().unwrap().since_ms;
    scheduled_start(&f, "cron-turn", "Scheduled self reminder");
    f.consume();
    f.status(RuntimeStatus::Working);
    f.stopped("cron-turn", "CRON_TICK");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_ne!(f.reply(lead).as_deref(), Some("CRON_TICK"));
    f.now = since + 60 * 60_000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Requested answer"));
}

#[test]
fn scheduled_prompt_waits_for_its_late_transcript_row_before_rebinding_a_hold() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "report progress", "turn-1");
    f.record(cron_stop("turn-1", "Requested answer"));
    f.consume();
    let since = f.request(lead).shell_hold.as_ref().unwrap().since_ms;
    let transcript = f.dir.join("scheduled.jsonl");
    std::fs::write(&transcript, "").unwrap();
    f.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "prompt_id":"cron-turn","prompt":"Scheduled self reminder", "transcript_path":transcript}),
    );
    f.consume();
    assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("turn-1"));
    // Claude writes this after its submit hooks run; the same spooled hook must retry.
    scheduled_start(&f, "cron-turn", "Scheduled self reminder");
    f.consume();
    f.record(cron_stop("cron-turn", "CRON_TICK"));
    f.consume();
    f.now = since + 15 * 60_000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Requested answer"));
}

#[test]
fn missing_prompt_origin_cannot_extend_the_captured_reply_cap() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "report progress", "turn-1");
    f.record(cron_stop("turn-1", "Requested answer"));
    f.consume();
    let since = f.request(lead).shell_hold.as_ref().unwrap().since_ms;
    f.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "prompt_id":"unknown-turn","prompt":"Unclassified wake",
        "transcript_path":f.dir.join("missing-transcript.jsonl")}),
    );
    f.consume();
    f.now = since + 15 * 60_000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Requested answer"));
    let next = f.send("next request", true);
    f.deliver();
    assert_eq!(f.request(next).phase, RequestPhase::Active);
}
