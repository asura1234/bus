use super::*;
use crate::messaging::model::{STEERING_SETTLE_MS, TURN_SETTLE_MS};

fn stamp(ms: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

fn recorded_turn(ms: u64, turn: &str, prompt: &str, answer: &str) -> String {
    let at = stamp(ms);
    [
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_started","turn_id":turn}}),
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"item_completed","turn_id":turn,
            "item":{"type":"UserMessage","content":[{"type":"text","text":prompt}]}}}),
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_complete","turn_id":turn,
            "last_agent_message":answer}}),
    ]
    .iter()
    .map(|line| format!("{line}\n"))
    .collect()
}

fn append_lines(path: &Path, lines: &str) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
}

fn idle_past_settle(fixture: &mut Fixture) {
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += TURN_SETTLE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
}

fn with_session_rollout() -> (Fixture, PathBuf) {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
    fixture.record(
        json!({"hook_event_name":"SessionStart","session_id":"session",
        "source":"startup","transcript_path":rollout}),
    );
    fixture.consume();
    (fixture, rollout)
}

/// An unbound request recovered from the rollout consumes that turn: a Stop
/// for the same turn arriving late must not become the next request's reply.
#[test]
fn a_late_stop_for_a_recovered_unbound_turn_is_not_the_next_request_reply() {
    let (mut fixture, rollout) = with_session_rollout();
    let first = fixture.send("copy the evidence", false);
    fixture.deliver();
    let typed = fixture.typed().last().unwrap().0.clone();
    append_lines(
        &rollout,
        &recorded_turn(fixture.now + 1, "turn-2", &typed, "Copied 374 files."),
    );
    idle_past_settle(&mut fixture);
    assert_eq!(fixture.reply(first).as_deref(), Some("Copied 374 files."));

    let next = fixture.send("now summarize", false);
    fixture.deliver();
    fixture.stopped("turn-2", "Copied 374 files.");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_ne!(
        fixture.reply(next).as_deref(),
        Some("Copied 374 files."),
        "the recovered turn's late Stop was delivered to the next request"
    );
}

/// Steered input that Codex ran as a turn of its own (its rollout shows the
/// prompt under a later turn) must not be answered with the lead turn's
/// recorded final when no hook ever bound either turn.
#[test]
fn steered_input_run_as_its_own_turn_does_not_take_the_lead_turn_recorded_answer() {
    let (mut fixture, rollout) = with_session_rollout();
    let lead = fixture.send("merge the PR", false);
    fixture.deliver();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"merge the PR","transcript_path":rollout}),
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    assert!(fixture.request(lead).trusted_start_bound);
    let follow = fixture.send("then close the worktree", false);
    fixture.deliver();
    assert_eq!(fixture.request(follow).group, Some(lead));
    let typed = fixture.typed().last().unwrap().0.clone();
    append_lines(
        &rollout,
        &(recorded_turn(fixture.now + 1, "turn-1", "merge the PR", "Merged.")
            + &recorded_turn(fixture.now + 2, "turn-2", &typed, "Closed the worktree.")),
    );
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += TURN_SETTLE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += STEERING_SETTLE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.reply(lead).as_deref(), Some("Merged."));
    assert_ne!(
        fixture.reply(follow).as_deref(),
        Some("Merged."),
        "steered input that ran as turn-2 took turn-1's recorded answer"
    );
    assert_eq!(fixture.request(follow).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture.request(follow).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
}

/// Steered input that Codex took into the running turn (its rollout records
/// the prompt under that turn) shares the turn's recovered answer.
#[test]
fn steered_input_recorded_in_the_lead_turn_shares_its_recorded_answer() {
    let (mut fixture, rollout) = with_session_rollout();
    let lead = fixture.send("merge the PR", false);
    fixture.deliver();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"merge the PR","transcript_path":rollout}),
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    let follow = fixture.send("then close the worktree", false);
    fixture.deliver();
    assert_eq!(fixture.request(follow).group, Some(lead));
    let typed = fixture.typed().last().unwrap().0.clone();
    let at = stamp(fixture.now + 1);
    let steered = json!({"timestamp":at,"type":"event_msg","payload":{"type":"item_completed",
        "turn_id":"turn-1","item":{"type":"UserMessage","content":[{"type":"text","text":typed}]}}});
    let turn = recorded_turn(
        fixture.now + 1,
        "turn-1",
        "merge the PR",
        "Merged and closed.",
    );
    let (head, complete) = turn.trim_end().rsplit_once('\n').unwrap();
    append_lines(&rollout, &format!("{head}\n{steered}\n{complete}\n"));
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += TURN_SETTLE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += STEERING_SETTLE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for id in [lead, follow] {
        assert_eq!(fixture.reply(id).as_deref(), Some("Merged and closed."));
    }
}

#[path = "codex_identical_steering_test.rs"]
mod codex_identical_steering_tests;

/// A steered follower that transcript recovery detaches from its group closes
/// as `reply_not_captured`; its no-reply diagnostic must still name the lead.
#[test]
fn reply_capture_diagnostics_record_a_follower_detached_by_transcript_recovery() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut fixture, rollout) = with_session_rollout();
        let lead = fixture.send("merge the PR", false);
        fixture.deliver();
        fixture.record(
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
            "turn_id":"turn-1","prompt":"merge the PR","transcript_path":rollout}),
        );
        fixture.consume();
        fixture.status(RuntimeStatus::Working);
        let follow = fixture.send("then close the worktree", false);
        fixture.deliver();
        let typed = fixture.typed().last().unwrap().0.clone();
        append_lines(
            &rollout,
            &(recorded_turn(fixture.now + 1, "turn-1", "merge the PR", "Merged.")
                + &recorded_turn(fixture.now + 2, "turn-2", &typed, "Closed the worktree.")),
        );
        fixture.status(RuntimeStatus::Working);
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        fixture.now += TURN_SETTLE_MS;
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        fixture.now += STEERING_SETTLE_MS;
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        assert_eq!(
            fixture.request(follow).failure_reason.as_deref(),
            Some("reply_not_captured")
        );
        let log = capture.text();
        let rows: Vec<_> = log
            .lines()
            .filter(|line| {
                line.contains("bus.message.recovered")
                    && line.contains("reason=\"reply_not_captured\"")
                    && line.contains(&format!("request_id={} ", follow.0))
            })
            .collect();
        assert_eq!(rows.len(), 1, "{log}");
        assert!(
            rows[0].contains(&format!("lead_id={}", lead.0)),
            "{}",
            rows[0]
        );
        assert!(
            !log.lines()
                .any(|line| line.contains("bus.message.recovered")
                    && line.contains("reason=\"reply_not_captured\"")
                    && line.contains(&format!("request_id={} ", lead.0))),
            "the recovered lead was answered, not abandoned: {log}"
        );
    });
}
