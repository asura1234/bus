use super::*;

// Real work keeps this folded-notification regression independent of dispatch classification.
#[test]
fn inherited_work_notification_folded_into_a_bound_turn_keeps_its_reply() {
    let mut f = master();
    f.consume();
    let lead = running(&mut f, "run a build", "turn-1");
    stop(&f, "turn-1", "Build running.", &[("build", "cargo build")]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    let second = f.send("new question", false);
    f.deliver();
    f.started("turn-2", "new question");
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Build running."));
    assert!(f.request(second).trusted_start_bound);
    f.started("turn-2", &notification("build"));
    f.consume();
    stop(&f, "turn-2", "New answer.", &[]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now += crate::messaging::model::TURN_SETTLE_MS + 1;
    f.consume();
    assert_eq!(f.reply(second).as_deref(), Some("New answer."));
    assert_eq!(f.request(second).failure_reason, None);
    assert_eq!(f.error(), None);
}

// Claude may report a wake under the original prompt id. The initial answer
// stays immutable; the later result is a separate follow-up.
#[test]
fn real_work_wake_under_the_held_prompt_id_still_delivers_its_result() {
    let mut f = master();
    f.consume();
    let lead = running(&mut f, "run the full test suite", "turn-1");
    stop(&f, "turn-1", "Running the suite.", &[("btest", REAL_WORK)]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.now += 60_000;
    f.status(RuntimeStatus::Working);
    f.started("turn-1", &notification("btest"));
    f.consume();
    stop(&f, "turn-1", "All 812 tests passed.", &[]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Running the suite."));
}

// A dispatch shell beside real work: neither wake can replace the already
// captured answer to the original request.
#[test]
fn dispatch_beside_real_work_still_delivers_the_work_result() {
    let mut f = master();
    f.consume();
    let lead = running(&mut f, "dispatch and test", "turn-1");
    stop(
        &f,
        "turn-1",
        "Dispatched; suite running.",
        &[("bdispatch", A_DISPATCH), ("btest", REAL_WORK)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.status(RuntimeStatus::Working);
    f.started("turn-2", &notification("bdispatch"));
    f.consume();
    stop(
        &f,
        "turn-2",
        "Dispatch acknowledged.",
        &[("btest", REAL_WORK)],
    );
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.now += 60_000;
    f.status(RuntimeStatus::Working);
    f.started("turn-3", &notification("btest"));
    f.consume();
    stop(&f, "turn-3", "All 812 tests passed.", &[]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Dispatched; suite running."));
}

// Claude may submit the finished shell's notification into the next turn
// before the developer's prompt; that prompt still binds and owns the final.
#[test]
fn inherited_notification_submitted_before_the_bound_prompt_keeps_its_reply() {
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
    let second = f.send("you should know which one you are", false);
    f.deliver();
    f.status(RuntimeStatus::Working);
    f.started("turn-2", &notification("bnn7n7h65"));
    f.consume();
    f.started("turn-2", "you should know which one you are");
    f.consume();
    stop(&f, "turn-2", "You're right. I'm desktop master.", &[]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now += crate::messaging::model::TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.reply(lead).as_deref(), Some("Given to worktree-closer."));
    assert_eq!(
        f.reply(second).as_deref(),
        Some("You're right. I'm desktop master.")
    );
    assert_eq!(f.request(second).failure_reason, None);
}
