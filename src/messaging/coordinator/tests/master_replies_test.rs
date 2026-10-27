//! Reproductions for the MASTER-room reply bugs of 2026-10-20 (A, B, C).
//! Mounted as a child of `steering_tests`, reusing that module's `Fixture`.
use super::*;

#[path = "lostreply_test.rs"]
mod lostreply_tests;

#[path = "master_reply_ownership_test.rs"]
mod ownership;

// Exact background commands from the incident transcripts.
const A_DISPATCH: &str = r#"/Users/dylanliu/work/bus/target/debug/bus send --room 13389 --as "mobile master" --to wt-close --async --text "Task: clean up the mobile (libtv-app) worktrees." >/dev/null; echo sent"#;
const C_DISPATCH_1: &str = r#"cd /Users/dylanliu/work/bus; target/debug/bus send --room 568 --as "bus master" --to fix-reply-lost --async --text "$(cat temp/briefs/fix-reply-lost.md)" > temp/briefs/fix-reply-lost.out.json 2>&1"#;
const C_DISPATCH_2: &str = r#"cd /Users/dylanliu/work/bus; target/debug/bus send --room 568 --as "bus master" --to fix-exit-snapshot --async --text "$(cat temp/briefs/fix-exit-snapshot.md)" > temp/briefs/fix-exit-snapshot.out.json 2>&1"#;
const C_RESUME: &str = r#"cd /Users/dylanliu/work/bus; target/debug/bus send --room 568 --as "bus master" --to fix-reply-lost --async --text "Your last turn was cut off by an API connection error (Connection lost mid-response). Continue the task." > temp/briefs/fix-reply-lost.out.json 2>&1"#;
const B_DISPATCH: &str = r#"/Users/dylanliu/work/bus/target/debug/bus send --room 1 --as "desktop master" --to worktree-closer --async --text "$(cat /private/tmp/claude-501/-Users-dylanliu-work-libtv-desktop/5313302c-1615-4863-8c5e-da4b37e493c0/scratchpad/wt-close-brief.txt)""#;
const B_REVISED: &str = r#"/Users/dylanliu/work/bus/target/debug/bus send --room 1 --as "desktop master" --to worktree-closer --async --text "Revised task, desktop only.""#;

fn stop(f: &Fixture, turn: &str, text: &str, shells: &[(&str, &str)]) {
    let tasks: Vec<_> = shells
        .iter()
        .map(|(id, command)| json!({"id":id,"type":"shell","status":"running","description":"Run","command":command}))
        .collect();
    f.record(
        json!({"hook_event_name":"Stop","session_id":"session","prompt_id":turn,
        "last_assistant_message":text,"background_tasks":tasks,"session_crons":[]}),
    );
}

fn notification(task: &str) -> String {
    format!("<task-notification>\n<task-id>{task}</task-id>\n<status>completed</status>\n</task-notification>")
}

fn master() -> Fixture {
    let f = Fixture::in_room(Provider::ClaudeCode, true);
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    f
}

fn diag(f: &Fixture, label: &str, id: RequestId) {
    let r = f.request(id);
    eprintln!(
        "{label}: phase={:?} failure={:?} turn={:?} awaiting_background={} reply={:?}",
        r.phase,
        r.failure_reason,
        r.provider_turn_id,
        r.awaiting_background,
        r.pending_final.as_ref().map(|p| p.text.clone())
    );
}

/// Bug A: an orchestrator's answer whose only background shell is its own
/// async `bus send` dispatch is held, so MASTER shows no reply at turn end.
#[test]
fn bug_a_master_answer_with_async_dispatch_shell_is_delivered_at_turn_end() {
    let mut f = master();
    f.consume();
    let lead = running(&mut f, "one of you does desktop, one mobile", "turn-1");
    stop(
        &f,
        "turn-1",
        "Mobile cleanup dispatched to wt-close.",
        &[("bf32h5qnq", A_DISPATCH)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.now += crate::messaging::model::TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    diag(&f, "A lead after Idle", lead);
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("Mobile cleanup dispatched to wt-close."),
        "the orchestrator's answer must reach MASTER when its turn ends"
    );
}

/// Bug C: the answer to a prompt (and the message folded into it) is replaced
/// by a later wake turn's final; both requests record the later text.
#[test]
fn bug_c_dispatch_answer_is_not_replaced_by_the_wake_turn_answer() {
    let mut f = master();
    f.consume();
    let lead = running(&mut f, "ok go dispatch this lost codex reply fix", "turn-1");
    let joined = f.send("go dispatch this to be worked on too", false);
    f.deliver();
    f.started("turn-1", "go dispatch this to be worked on too");
    f.consume();
    assert_eq!(f.request(joined).group, Some(lead));

    stop(
        &f,
        "turn-1",
        "Both fixes are dispatched.",
        &[("b14lywcl1", C_DISPATCH_1), ("byw4xut8j", C_DISPATCH_2)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    diag(&f, "C lead after first answer", lead);

    // b14lywcl1 finishes and wakes a turn of its own.
    f.status(RuntimeStatus::Working);
    f.started("turn-2", &notification("b14lywcl1"));
    f.consume();
    stop(
        &f,
        "turn-2",
        "`fix-reply-lost` lost its connection.",
        &[("byw4xut8j", C_DISPATCH_2), ("bttaarmtt", C_RESUME)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    diag(&f, "C lead after wake turn", lead);

    // The developer's next message starts a new turn and releases the hold.
    let next = f.send("next question", false);
    f.deliver();
    f.status(RuntimeStatus::Working);
    f.started("turn-3", "next question");
    f.consume();
    diag(&f, "C lead after next prompt", lead);
    diag(&f, "C joined after next prompt", joined);
    diag(&f, "C next", next);
    for id in [lead, joined] {
        assert_eq!(
            f.reply(id).as_deref(),
            Some("Both fixes are dispatched."),
            "request {id:?} must keep the answer to its own prompt"
        );
    }
}

/// Bug B: a finished background shell's task notification, absorbed into the
/// running turn of the next request (same prompt id), marks that turn
/// unrelated; its real final is then rejected and the request abandoned.
#[test]
fn bug_b_task_notification_absorbed_mid_turn_does_not_lose_the_reply() {
    let mut f = master();
    f.consume();
    let lead = running(&mut f, "close merged worktrees", "turn-1");
    stop(
        &f,
        "turn-1",
        "Given to worktree-closer.",
        &[("bnn7n7h65", B_DISPATCH)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    diag(&f, "B first request after its turn", lead);

    let second = f.send("you should know which one you are", false);
    f.deliver();
    f.status(RuntimeStatus::Working);
    f.started("turn-2", "you should know which one you are");
    f.consume();
    diag(&f, "B first request after second prompt", lead);
    diag(&f, "B second request bound", second);

    // Claude folds the dispatch shell's notification into turn-2 and reports
    // it under turn-2's prompt id.
    f.started("turn-2", &notification("bnn7n7h65"));
    f.consume();
    stop(
        &f,
        "turn-2",
        "You're right. I'm desktop master.",
        &[("bsb7esmss", B_REVISED)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.now += crate::messaging::model::TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    diag(&f, "B second request final", second);
    assert_eq!(
        f.reply(second).as_deref(),
        Some("You're right. I'm desktop master."),
        "the second request's own turn carried its reply"
    );
    assert_eq!(f.request(second).failure_reason, None);
}

// ---- Verification of the fix (2026-10-21): paths the change could break. ----

const REAL_WORK: &str = "cargo test --workspace 2>&1 | tail -5";

fn notice_texts(f: &Fixture, room: RoomId) -> Vec<String> {
    f.worker
        .state
        .notices(room)
        .map(|notice| notice.text.clone())
        .collect()
}

/// A request whose turn waits on a real long shell, then the shell's wake turn
/// reports the result. Returns the request, and its phase right after turn-1.
fn real_shell_then_wake(f: &mut Fixture) -> (RequestId, RequestPhase) {
    f.consume();
    let lead = running(f, "run the full test suite", "turn-1");
    stop(
        f,
        "turn-1",
        "Running the suite in the background.",
        &[("btest", REAL_WORK)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    let held_phase = f.request(lead).phase;
    f.now += 4 * 60_000;
    f.status(RuntimeStatus::Working);
    f.started("turn-2", &notification("btest"));
    f.consume();
    stop(f, "turn-2", "All 812 tests passed.", &[]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    (lead, held_phase)
}

/// Worker in a work room, developer asks directly; the turn's only real
/// answer comes from the wake turn after the background suite finishes.
#[test]
fn verify_worker_stop_answer_is_not_replaced_by_a_background_result() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    let (lead, held_phase) = real_shell_then_wake(&mut f);
    diag(&f, "V worker after wake turn", lead);
    eprintln!("V worker held phase after turn-1: {held_phase:?}");
    eprintln!("V worker room notices: {:?}", notice_texts(&f, f.room));
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("Running the suite in the background.")
    );
}

/// Same in MASTER: the developer's MASTER message waits on a real shell.
#[test]
fn verify_master_stop_answer_is_not_replaced_by_a_background_result() {
    let mut f = master();
    let (lead, held_phase) = real_shell_then_wake(&mut f);
    diag(&f, "V master after wake turn", lead);
    eprintln!("V master held phase after turn-1: {held_phase:?}");
    eprintln!("V master room notices: {:?}", notice_texts(&f, f.room));
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("Running the suite in the background.")
    );
}

/// A folded task notification cannot change a final already delivered.
#[test]
fn a_folded_task_notification_preserves_an_already_captured_final() {
    let mut f = master();
    f.consume();
    let lead = running(&mut f, "report the result", "turn-1");
    stop(&f, "turn-1", "The build passed.", &[]);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Completed);
    assert!(f.request(lead).reply().is_some());

    f.started("turn-1", &notification("older-dispatch"));
    f.consume();
    assert_eq!(
        f.request(lead).reply().map(|reply| reply.text.as_str()),
        Some("The build passed.")
    );
    assert_eq!(f.error(), None);
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("The build passed."));
}

#[test]
fn opaque_shell_syntax_cannot_delay_or_replace_the_stop_answer() {
    let command =
        "bus send --to worker --async --text \"$(cat brief.md # 1) task context\ncargo test\n)\"";
    let mut f = master();
    f.consume();
    let lead = running(&mut f, "run and report the tests", "turn-1");
    stop(
        &f,
        "turn-1",
        "Running the tests in the background.",
        &[("tests-and-dispatch", command)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(
        f.request(lead).phase,
        RequestPhase::Completed,
        "opaque shell syntax must not delay a captured answer"
    );
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("Running the tests in the background.")
    );

    f.status(RuntimeStatus::Working);
    f.started("turn-2", &notification("tests-and-dispatch"));
    f.consume();
    stop(&f, "turn-2", "All tests passed.", &[]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("Running the tests in the background.")
    );
}
