use super::*;

// Real work keeps these failures independent of dispatch-shell classification.
#[test]
fn a_background_wake_cannot_rebind_a_request_with_a_captured_reply() {
    for wake_shells in [vec![], vec![("build", "cargo build")]] {
        let mut f = master();
        f.consume();
        let lead = running(&mut f, "build it", "turn-1");
        let joined = f.send("and check it", false);
        f.deliver();
        f.started("turn-1", "and check it");
        f.consume();
        stop(
            &f,
            "turn-1",
            "Build dispatched.",
            &[("build", "cargo build")],
        );
        f.consume();
        f.status(RuntimeStatus::Idle);
        let since = f.request(lead).shell_hold.as_ref().unwrap().since_ms;
        f.started("wake-turn", &notification("build"));
        f.consume();
        f.status(RuntimeStatus::Working);
        stop(&f, "wake-turn", "Wake report.", &wake_shells);
        f.consume();
        f.status(RuntimeStatus::Idle);
        f.now = since + 15 * 60_000;
        f.status(RuntimeStatus::Idle);
        f.consume();
        for id in [lead, joined] {
            assert_eq!(f.reply(id).as_deref(), Some("Build dispatched."));
            assert_eq!(f.request(id).provider_turn_id.as_deref(), Some("turn-1"));
        }
    }
}

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
