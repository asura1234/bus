use super::*;

/// A status read between coordinator ticks must materialize the stalled fact
/// before returning the exact keys its author waiter will print and acknowledge.
/// Once that waiter exits, expiry of its lease must not cause another wake.
#[test]
fn stall_printed_by_the_author_waiter_does_not_wake_the_orchestrator_again() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    let source = earlier_background_request_at(
        &mut f,
        false,
        now - crate::messaging::model::DELIVERED_STALL_MS - 5_000,
    );
    // The last tick before the stall threshold reported the idle open request.
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(now - 20_000);
    f.worker.save(state).unwrap();
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let status = f.call(
        "message.status",
        json!({"message":message.0.to_string(),"as":f.orch.0.to_string()}),
    );
    let row = status.result["requests"][0].clone();
    assert_eq!(row["stage"], "stalled", "the waiter exits on this stall");
    let keys = row["event_acks"].clone();
    if !keys.as_array().unwrap().is_empty() {
        assert!(
            f.call(
                "message.seen",
                json!({"request":source.0.to_string(),"as":f.orch.0.to_string(),"events":keys})
            )
            .ok
        );
    }
    f.worker.reply_waiters.clear();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert!(
        !calls.iter().any(|text| text.contains("No reply (stalled")),
        "the stall the waiter already printed was delivered again: {calls:?}"
    );
}

/// A coordinator tick reconciles outcomes in `expire_queued_now` before
/// `settle_requests` closes a turn that ended without a reply. The first tick
/// past the settle window therefore sees the request idle and still open.
#[test]
fn reply_not_captured_settlement_folds_the_idle_notice_into_its_outcome() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 6_000)
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.expire_queued_now().unwrap();
    let mut state = f.worker.state.clone();
    assert_eq!(
        state.settle_ended_request(f.agent, io::now_ms(), |_| None),
        Some(source)
    );
    f.worker.save(state).unwrap();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("reply_not_captured"));
    assert!(
        !calls[0].contains("Idle with open request"),
        "idle followed by no-reply settlement must fold into the outcome: {calls:?}"
    );
}

/// Same tick order with the author's `send --async` waiter alive: the waiter
/// prints the `reply_not_captured` outcome, so no separate terminal wake may
/// still claim the reply is pending.
#[test]
fn reply_not_captured_under_a_live_author_waiter_sends_no_stale_idle_wake() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 6_000)
        .unwrap();
    f.worker.save(state).unwrap();
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":orch.0.to_string()})
        )
        .ok
    );
    let mut state = f.worker.state.clone();
    assert_eq!(
        state.settle_ended_request(f.agent, io::now_ms(), |_| None),
        Some(source)
    );
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    let calls = f.calls.lock().unwrap();
    assert!(
        !calls
            .iter()
            .any(|text| text.contains("Idle with open request (reply pending)")),
        "a closed request was announced as still pending: {calls:?}"
    );
}
