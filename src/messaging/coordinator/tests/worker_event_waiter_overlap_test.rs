use super::*;

/// A status read returns the actual request stage with no outcome to
/// acknowledge, and expiry of the author waiter's lease causes no wake.
#[test]
fn delivered_status_has_no_outcome_ack_or_orchestrator_wake() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    let source = earlier_background_request_at(
        &mut f,
        false,
        now - crate::messaging::model::DELIVERED_STALL_MS - 5_000,
    );
    // A tick before the recovery deadline.
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(now - 20_000);
    f.worker.save(state).unwrap();
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let status = f.call(
        "message.status",
        json!({"message":message.0.to_string(),"as":f.orch.0.to_string()}),
    );
    let row = status.result["requests"][0].clone();
    assert_eq!(row["stage"], "delivered");
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
        !calls.iter().any(|text| text.contains("No reply (")),
        "a delivered status produced an outcome wake: {calls:?}"
    );
}

/// Idle plus the settle grace, with no recorded end, completes the turn and
/// does not post a no-reply outcome. The agent is spare again.
#[test]
fn reply_not_captured_settlement_sends_one_outcome() {
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
    let settled = state.request(source).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert!(settled.reply().is_none(), "{settled:?}");
    assert!(settled.failure_reason.is_none(), "{settled:?}");
    assert_eq!(settled.delivery_stage(), "ended");
    assert_eq!(state.agent(f.agent).unwrap().current_request, None);
    f.worker.save(state).unwrap();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .all(|call| !call.contains("reply_not_captured") && !call.contains("No reply")),
        "{calls:?}"
    );
}
