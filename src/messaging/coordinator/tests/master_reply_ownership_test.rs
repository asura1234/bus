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
