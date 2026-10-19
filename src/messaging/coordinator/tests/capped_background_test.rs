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
        assert_eq!(f.reply(lead).as_deref(), Some("Latest report"));
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
