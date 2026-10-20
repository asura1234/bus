//! Adversarial attribution cases for the unbound rollout fallback, adopted
//! from the bug-reply-not-captured verifier (2026-10-20, rounds 1 and 2):
//! an unbound request takes a rollout answer only when the one turn that ran
//! its prompt since the send can be proven; otherwise `reply_not_captured`.
use super::*;
use crate::agents::providers::codex::usage::{
    find_codex_prompt_turn, read_codex_turn_end, CodexTurnEnd,
};
use crate::messaging::model::TURN_SETTLE_MS;
use std::io::Write;

const SESSION: &str = "01a157b4-679a-7cb1-b32d-81696ceafe47";

fn with_rollout() -> (Fixture, PathBuf) {
    let mut f = Fixture::new(Provider::Codex);
    let path = f.dir.join("rollout-independent-verification.jsonl");
    let worker_cwd = f.dir.join("worker-worktree");
    std::fs::create_dir(&worker_cwd).unwrap();
    let mut state = f.worker.state.clone();
    state
        .update_agent_runtime_metadata(f.agent, worker_cwd, None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            f.agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some(SESSION.into()),
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    f.record(json!({"hook_event_name":"SessionStart","session_id":SESSION,"source":"startup","transcript_path":path}));
    f.consume();
    (f, path)
}

fn idle_then_settle(f: &mut Fixture, id: RequestId) {
    f.status(RuntimeStatus::Working);
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(
        f.request(id).phase,
        RequestPhase::Active,
        "reply must wait for settle grace"
    );
    f.now += TURN_SETTLE_MS;
    f.status(RuntimeStatus::Idle);
    f.consume();
}

fn at(ms: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

fn turn(id: &str, ms: u64, prompt: &str, answer: &str) -> String {
    let timestamp = at(ms);
    [
        json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"task_started","turn_id":id}}),
        json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"item_completed","turn_id":id,"item":{"type":"UserMessage","content":[{"type":"text","text":prompt}]}}}),
        json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"task_complete","turn_id":id,"last_agent_message":answer}}),
    ].iter().map(|value| format!("{value}\n")).collect()
}

fn no_answer(f: &Fixture, id: RequestId) {
    assert_eq!(f.request(id).phase, RequestPhase::Abandoned);
    assert_eq!(
        f.request(id).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    assert!(f.reply(id).is_none());
}

fn bound(f: &mut Fixture, path: &Path, turn_id: &str, prompt: &str) -> RequestId {
    let id = f.send(prompt, false);
    f.deliver();
    f.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":SESSION,
        "turn_id":turn_id,"prompt":prompt,"transcript_path":path}),
    );
    f.consume();
    assert!(f.request(id).trusted_start_bound);
    id
}

#[test]
fn send_boundary_is_inclusive_and_a_one_ms_older_turn_is_rejected() {
    for offset in [-1_i64, 0, 1] {
        let (mut f, path) = with_rollout();
        let id = f.send("boundary", false);
        f.deliver();
        let since = f.request(id).prompt.submitted_at_ms;
        let stamp = (since as i64 + offset) as u64;
        std::fs::write(&path, turn("candidate", stamp, "boundary", "RIGHT")).unwrap();
        idle_then_settle(&mut f, id);
        if offset < 0 {
            no_answer(&f, id);
        } else {
            assert_eq!(f.reply(id).as_deref(), Some("RIGHT"));
        }
    }
}

#[test]
fn two_matching_turns_in_the_send_millisecond_remain_ambiguous() {
    let (mut f, path) = with_rollout();
    let id = f.send("same-ms", false);
    f.deliver();
    let since = f.request(id).prompt.submitted_at_ms;
    std::fs::write(
        &path,
        turn("one", since, "same-ms", "ONE") + &turn("two", since, "same-ms", "TWO"),
    )
    .unwrap();
    idle_then_settle(&mut f, id);
    no_answer(&f, id);
}

#[test]
fn a_partially_written_final_never_delivers_its_partial_text() {
    let (mut f, path) = with_rollout();
    let id = f.send("partial", false);
    f.deliver();
    let since = f.request(id).prompt.submitted_at_ms;
    let recorded = turn("candidate", since + 1, "partial", "NOT COMPLETE");
    let lines: Vec<_> = recorded.lines().collect();
    let prefix = format!("{}\n{}\n", lines[0], lines[1]);
    let partial = lines[2].split("NOT COMPLETE").next().unwrap();
    std::fs::write(&path, prefix + partial + "NOT CO").unwrap();
    idle_then_settle(&mut f, id);
    no_answer(&f, id);
}

#[test]
fn complete_final_json_without_a_trailing_newline_delivers_only_its_exact_text() {
    let (mut f, path) = with_rollout();
    let id = f.send("no-newline", false);
    f.deliver();
    let since = f.request(id).prompt.submitted_at_ms;
    std::fs::write(
        &path,
        turn("candidate", since + 1, "no-newline", "EXACT").trim_end_matches('\n'),
    )
    .unwrap();
    idle_then_settle(&mut f, id);
    assert_eq!(f.reply(id).as_deref(), Some("EXACT"));
}

#[test]
fn a_renamed_rollout_and_an_unregistered_new_file_deliver_no_foreign_answer() {
    for is_bound in [false, true] {
        let (mut f, path) = with_rollout();
        let id = if is_bound {
            bound(&mut f, &path, "original", "rotation")
        } else {
            let id = f.send("rotation", false);
            f.deliver();
            id
        };
        let since = f.request(id).prompt.submitted_at_ms;
        std::fs::write(&path, turn("original", since + 1, "rotation", "ORIGINAL")).unwrap();
        std::fs::rename(&path, f.dir.join("archived-rollout.jsonl")).unwrap();
        // A new Codex session's file is not the rollout registered by the
        // old session hook. No hook reports it to Bus in this missing-hook case.
        let new_path = f.dir.join("rollout-new-session.jsonl");
        let header =
            json!({"timestamp":at(since + 2),"type":"session_meta","payload":{"id":"new-session"}});
        std::fs::write(
            new_path,
            format!("{header}\n") + &turn("new-turn", since + 2, "rotation", "FOREIGN"),
        )
        .unwrap();
        idle_then_settle(&mut f, id);
        no_answer(&f, id);
    }
}

#[test]
fn a_bound_completion_outside_the_tail_never_uses_a_later_turn_answer() {
    let (mut f, path) = with_rollout();
    let id = bound(&mut f, &path, "bound-turn", "bound");
    let since = f.request(id).prompt.submitted_at_ms;
    let bytes = turn("bound-turn", since + 1, "bound", "BOUND")
        + &json!({"timestamp":at(since + 2),"type":"event_msg","payload":{"type":"token_count","padding":"x".repeat(300*1024)}}).to_string()
        + "\n" + &turn("later", since + 3, "bound", "WRONG");
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(read_codex_turn_end(&path, "bound-turn").unwrap(), None);
    assert_eq!(
        read_codex_turn_end(&path, "later").unwrap(),
        Some(CodexTurnEnd::Final("WRONG".into()))
    );
    idle_then_settle(&mut f, id);
    no_answer(&f, id);
}

#[test]
fn a_window_over_64_mib_refuses_attribution_instead_of_using_the_later_match() {
    let (mut f, path) = with_rollout();
    let id = f.send("cap", false);
    f.deliver();
    let since = f.request(id).prompt.submitted_at_ms;
    let initial = turn("first", since + 1, "cap", "FIRST");
    let parts: Vec<_> = initial.lines().collect();
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, "{}\n{}", parts[0], parts[1]).unwrap();
    write!(file, "{{\"timestamp\":\"{}\",\"type\":\"response_item\",\"payload\":{{\"type\":\"reasoning\",\"encrypted_content\":\"", at(since + 2)).unwrap();
    let block = vec![b'x'; 1024 * 1024];
    for _ in 0..65 {
        file.write_all(&block).unwrap();
    }
    writeln!(file, "\"}}}}").unwrap();
    writeln!(file, "{}", parts[2]).unwrap();
    file.write_all(turn("later", since + 3, "cap", "LATER").as_bytes())
        .unwrap();
    drop(file);
    assert!(std::fs::metadata(&path).unwrap().len() > 64 * 1024 * 1024);
    assert_eq!(
        find_codex_prompt_turn(&path, since, |text| text == "cap").unwrap(),
        None
    );
    idle_then_settle(&mut f, id);
    no_answer(&f, id);
}

#[test]
fn an_unbound_long_turn_with_one_matching_prompt_still_delivers_its_own_final() {
    let (mut f, path) = with_rollout();
    let id = f.send("long", false);
    f.deliver();
    let since = f.request(id).prompt.submitted_at_ms;
    let initial = turn("candidate", since + 1, "long", "EXACT LONG FINAL");
    let parts: Vec<_> = initial.lines().collect();
    let filler = json!({"timestamp":at(since + 2),"type":"response_item","payload":{"type":"reasoning","encrypted_content":"x".repeat(1024*1024)}});
    std::fs::write(
        &path,
        format!("{}\n{}\n{filler}\n{}\n", parts[0], parts[1], parts[2]),
    )
    .unwrap();
    idle_then_settle(&mut f, id);
    assert_eq!(f.reply(id).as_deref(), Some("EXACT LONG FINAL"));
}

#[test]
fn clock_rollback_near_send_must_not_hide_an_earlier_matching_turn() {
    for rollback in [false, true] {
        let (mut f, path) = with_rollout();
        let id = f.send("clock", false);
        f.deliver();
        let since = f.request(id).prompt.submitted_at_ms;
        let initial = turn("first", since + 1, "clock", "FIRST");
        let parts: Vec<_> = initial.lines().collect();
        let filler = json!({"timestamp":at(since + 2),"type":"response_item","payload":{"type":"reasoning","encrypted_content":"x".repeat(300*1024)}});
        // First run a monotonic control; then change only this completion's
        // timestamp, simulating a 3 ms wall-clock rollback (+2 to -1).
        // All lines remain appended in order. Both matching task_started
        // timestamps remain after the Bus send. No system clock is changed.
        let mut complete: Value = serde_json::from_str(parts[2]).unwrap();
        complete["timestamp"] = json!(at(if rollback { since - 1 } else { since + 2 }));
        let recorded = format!("{}\n{}\n{filler}\n{complete}\n", parts[0], parts[1])
            + &turn("later", since + 1_000, "clock", "LATER");
        std::fs::write(&path, recorded).unwrap();
        let selected = find_codex_prompt_turn(&path, since, |text| text == "clock").unwrap();
        f.now = since + 1_001;
        idle_then_settle(&mut f, id);
        println!(
            "CLOCK ROLLBACK={rollback}: selected={selected:?}; phase={:?}; reply={:?}",
            f.request(id).phase,
            f.reply(id)
        );
        assert!(f.reply(id).is_none(), "an append-only rollout with two matching post-send starts must remain ambiguous despite clock rollback");
    }
}
