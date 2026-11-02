use super::*;
use crate::messaging::coordinator::tests::support::{fixture, native_info, queue};
use crate::protocol::api::schema::{AgentStatus, ResponseResult};

const ERROR: &str = "429 You've reached your 5-hour usage limit. Your quota will reset when the current 5-hour window ends. (provider.api_error)";

fn limited_info() -> schema::AgentInfo {
    let mut info = native_info("pane", "kimi", "session");
    info.agent_status = AgentStatus::Unknown;
    info.provider_error = Some(ERROR.into());
    info.interactive_ready = true;
    info
}

#[test]
fn kimi_quota_queued_message_is_probed_once_after_backoff_and_recovers() {
    let mut healthy = native_info("pane", "kimi", "session");
    healthy.interactive_ready = true;
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Kimi,
        vec![
            Ok(ResponseResult::AgentPrompted {
                agent: healthy.clone(),
            }),
            Ok(ResponseResult::AgentList {
                agents: vec![healthy],
            }),
        ],
    );
    let now = io::now_ms();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Unavailable, now)
        .unwrap();
    worker.kimi_quota.now_ms = Some(now);
    worker.observe_kimi_quotas(&[limited_info()]);
    let request = queue(&mut worker, room, agent, "deliver once after quota reset");

    worker.submit_ready().unwrap();
    assert!(
        calls.lock().unwrap().is_empty(),
        "a quota observation starts a backoff"
    );
    worker.kimi_quota.now_ms = Some(now + RETRY_AFTER_MS - 1);
    worker.observe_kimi_quotas(&[limited_info()]);
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        super::super::RequestPhase::Queued
    );

    worker.kimi_quota.now_ms = Some(now + RETRY_AFTER_MS);
    worker.expire_queued_requests(now + RETRY_AFTER_MS).unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|name| **name == "agent.prompt_if_idle")
            .count(),
        1
    );
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    assert!(worker.state.queued_requests(agent).is_empty());

    worker.kimi_quota.now_ms = Some(now + 3 * RETRY_AFTER_MS);
    worker.observe_kimi_quotas(&[limited_info()]);
    worker.submit_ready().unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|name| **name == "agent.prompt_if_idle")
            .count(),
        1,
        "an accepted or uncertain request must never become another quota probe"
    );
    worker.tick().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Idle
    );
    assert!(
        worker.kimi_quota.retries.is_empty(),
        "fresh healthy native evidence clears the hold"
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|name| **name == "agent.prompt_if_idle")
            .count(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn kimi_quota_probe_does_not_bypass_other_unavailability_or_foreign_identity() {
    for (provider, error, terminal) in [
        (Provider::Kimi, ERROR, "foreign"),
        (
            Provider::Kimi,
            "429 Too many requests; retry in one second",
            "terminal",
        ),
        (Provider::Codex, ERROR, "terminal"),
    ] {
        let (mut worker, agent, room, dir, calls) = fixture(provider, vec![]);
        let now = io::now_ms();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Unavailable, now)
            .unwrap();
        let mut info = native_info(
            "pane",
            crate::agents::providers::launch::provider_kind(provider),
            "session",
        );
        info.agent_status = AgentStatus::Unknown;
        info.provider_error = Some(error.into());
        info.terminal_id = terminal.into();
        info.interactive_ready = true;
        worker.kimi_quota.now_ms = Some(now);
        worker.observe_kimi_quotas(&[info]);
        let request = queue(&mut worker, room, agent, "retain while unavailable");
        worker.kimi_quota.now_ms = Some(now + RETRY_AFTER_MS);
        worker.submit_ready().unwrap();
        assert!(
            calls.lock().unwrap().is_empty(),
            "{provider:?} {error} {terminal}"
        );
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            super::super::RequestPhase::Queued
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
