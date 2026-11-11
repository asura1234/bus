use super::*;
use crate::messaging::control::{Request as ControlRequest, Response};
use crate::messaging::model::{ExitSnapshot, TURN_SETTLE_MS};
use std::io::Write;

fn workspace() -> (Fixture, PathBuf, PathBuf) {
    let mut f = Fixture::new(Provider::Codex);
    let cwd = f.dir.join("launch-worktree");
    std::fs::create_dir(&cwd).unwrap();
    let rollout = f.dir.join("rollout-absolute-owned.jsonl");
    std::fs::write(&rollout, "").unwrap();
    let mut state = f.worker.state.clone();
    state
        .update_agent_runtime_metadata(f.agent, cwd.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    (f, cwd, rollout)
}

fn started() -> (Fixture, PathBuf, PathBuf, RequestId) {
    let (mut f, cwd, rollout) = workspace();
    let lead = f.send("finish the valid work", false);
    f.deliver();
    f.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"living-turn","prompt":"finish the valid work","transcript_path":rollout}),
    );
    f.consume();
    f.status(RuntimeStatus::Working);
    assert_eq!(f.worker.codex_rollouts.get(&f.agent), Some(&rollout));
    assert!(f.request(lead).trusted_start_bound);
    assert_eq!(f.request(lead).phase, RequestPhase::Active);
    (f, cwd, rollout, lead)
}

fn reply(f: &mut Fixture, request: RequestId, text: &str) -> Response {
    let message = f.request(request).prompt.id;
    f.worker.control_response_with_events(&ControlRequest {
        id: format!("late-reply-{}", request.0), method: "message.send".into(),
        params: json!({"reply_to":message.0.to_string(),"as":f.agent.0.to_string(),"text":text}),
    }, None)
}

#[test]
fn removed_directory_running_group_can_reply_after_every_detection_path() {
    for detection in ["admission", "sampler", "hook_failure", "settlement"] {
        let (mut f, cwd, _, lead) = started();
        let member = f.send("finish the grouped instruction too", false);
        f.deliver();
        assert_eq!(f.request(member).group, Some(lead));
        let queued = f.send("not yet started", true);
        std::fs::remove_dir(&cwd).unwrap();
        match detection {
            "admission" => {
                f.worker.check_launch_directory(f.agent).unwrap().unwrap();
            }
            "hook_failure" => {
                assert!(f
                    .worker
                    .agent_error(
                        f.agent,
                        "Hook failed: No such file or directory (os error 2)".into()
                    )
                    .is_err());
            }
            "settlement" => {
                f.worker.settle_requests().unwrap();
            }
            "sampler" => {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while f
                    .worker
                    .state
                    .agent(f.agent)
                    .unwrap()
                    .working_directory_error
                    .is_none()
                {
                    f.worker.directory_checks.poll(&mut f.worker.state).unwrap();
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::yield_now();
                }
            }
            _ => unreachable!(),
        }
        let answer = reply(&mut f, member, "late grouped result from the main checkout");
        assert!(answer.ok, "{detection}: {answer:?}");
        assert_eq!(answer.result["stage"], "replied");
        assert_eq!(
            f.reply(member).as_deref(),
            Some("late grouped result from the main checkout")
        );
        assert_eq!(f.request(lead).phase, RequestPhase::Active);
        assert!(f.request(lead).failure_reason.is_none());
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
        assert_eq!(f.request(queued).phase, RequestPhase::Abandoned);
        assert_eq!(
            f.request(queued).failure_reason.as_deref(),
            Some("launch_directory_removed")
        );
        assert!(f.worker.state.queued_requests(f.agent).is_empty());
        assert!(reply(&mut f, lead, "late original result").ok);
        let after = f.worker.state.clone();
        f.worker.check_launch_directory(f.agent).unwrap().unwrap();
        assert_eq!(
            f.worker.state, after,
            "repeat detection preserves captured replies and state"
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().shown_status(),
            RuntimeStatus::Unavailable
        );
    }
}

#[test]
fn removed_directory_idle_agent_refuses_new_send_without_persisting_it() {
    let (mut f, cwd, _) = workspace();
    std::fs::remove_dir(&cwd).unwrap();
    let started = std::time::Instant::now();
    let response = f.worker.control_response_with_events(&ControlRequest {
        id: "missing-directory-idle-admission".into(), method: "message.send".into(),
        params: json!({"room":f.room.0.to_string(),"to":[f.agent.0.to_string()],"text":"must not queue"}),
    }, None);
    assert!(!response.ok, "{response:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(response
        .error
        .unwrap()
        .message
        .contains("replace the agent"));
    assert_eq!(f.worker.state.requests().count(), 0);
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .current_request
        .is_none());
    assert!(f.worker.state.queued_requests(f.agent).is_empty());
}

#[test]
fn removed_directory_no_reply_turn_settles_from_absolute_owned_rollout() {
    let (mut f, cwd, rollout, lead) = started();
    std::fs::remove_dir(&cwd).unwrap();
    f.worker.check_launch_directory(f.agent).unwrap().unwrap();
    assert!(rollout.is_absolute() && rollout.exists());
    writeln!(std::fs::OpenOptions::new().append(true).open(&rollout).unwrap(), "{}",
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"living-turn","last_agent_message":"native final is not a Bus reply"}})).unwrap();
    assert!(
        crate::agents::providers::codex::usage::read_codex_turn_end(&rollout, "living-turn")
            .unwrap()
            .is_some()
    );
    // The Stop hook is gone; terminal polling may still show Working.
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().status,
        RuntimeStatus::Working
    );
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Completed);
    assert!(f.request(lead).failure_reason.is_none());
    assert!(f.reply(lead).is_none());
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .current_request
        .is_none());
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().shown_status(),
        RuntimeStatus::Unavailable
    );
}

#[test]
fn removed_directory_unverified_idle_does_not_end_the_bound_turn() {
    let (mut f, cwd, _, lead) = started();
    std::fs::remove_dir(&cwd).unwrap();
    f.worker.settle_requests().unwrap();
    f.status(RuntimeStatus::Idle);
    f.now += TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Active);
    assert!(f.request(lead).failure_reason.is_none());
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(lead)
    );
    assert!(
        reply(
            &mut f,
            lead,
            "valid answer after a misleading Idle observation"
        )
        .ok
    );
}

#[test]
fn removed_directory_unverified_unavailable_is_not_process_exit() {
    let (mut f, cwd, _, lead) = started();
    std::fs::remove_dir(&cwd).unwrap();
    f.worker.check_launch_directory(f.agent).unwrap().unwrap();
    f.status(RuntimeStatus::Unavailable);
    f.now += TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Unavailable);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Active);
    assert!(reply(&mut f, lead, "reply after native socket loss").ok);
}

#[test]
fn removed_directory_proved_process_exit_closes_only_unanswered_work() {
    let (mut f, cwd, _, lead) = started();
    let member = f.send("grouped result", false);
    f.deliver();
    assert!(reply(&mut f, member, "captured before exit").ok);
    std::fs::remove_dir(&cwd).unwrap();
    f.worker.check_launch_directory(f.agent).unwrap().unwrap();
    f.status(RuntimeStatus::Unavailable);
    let mut snapshot = ExitSnapshot {
        launch_id: Some("old-launch".into()),
        terminal_id: "terminal".into(),
        path: None,
        error: Some("verified native process exit".into()),
        tail: String::new(),
        at_ms: f.now,
        terminal_closed: Some(true),
    };
    let mut state = f.worker.state.clone();
    state
        .record_exit_snapshot(f.agent, snapshot.clone())
        .unwrap();
    f.worker.save(state).unwrap();
    f.now += TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Unavailable);
    f.consume();
    assert_eq!(
        f.request(lead).phase,
        RequestPhase::Active,
        "old launch exit is irrelevant"
    );
    snapshot.launch_id = Some("launch".into());
    snapshot.at_ms = f.now;
    let mut state = f.worker.state.clone();
    state.record_exit_snapshot(f.agent, snapshot).unwrap();
    f.worker.save(state).unwrap();
    f.consume();
    assert_eq!(
        f.request(lead).phase,
        RequestPhase::Active,
        "keep native exit grace"
    );
    f.now += TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Unavailable);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("agent_unavailable")
    );
    assert_eq!(f.reply(member).as_deref(), Some("captured before exit"));
    assert!(f.request(member).failure_reason.is_none());
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .current_request
        .is_none());
}

#[test]
fn removed_directory_transient_error_and_process_exit_close_unanswered_work() {
    let (mut f, cwd, rollout, lead) = started();
    let member = f.send("unanswered grouped request", false);
    f.deliver();
    assert_eq!(f.request(member).group, Some(lead));
    std::fs::remove_dir(&cwd).unwrap();
    f.worker.check_launch_directory(f.agent).unwrap().unwrap();
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap(),
        "{}",
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"living-turn",
            "error":{"message":"HTTP 503 Service Unavailable"}}})
    )
    .unwrap();
    f.status(RuntimeStatus::Unavailable);
    let mut state = f.worker.state.clone();
    state
        .record_exit_snapshot(
            f.agent,
            ExitSnapshot {
                launch_id: Some("launch".into()),
                terminal_id: "terminal".into(),
                path: None,
                error: Some("verified native process exit".into()),
                tail: String::new(),
                at_ms: f.now,
                terminal_closed: Some(true),
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    f.now += TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Unavailable);
    f.consume();
    for id in [lead, member] {
        assert_eq!(f.request(id).phase, RequestPhase::Abandoned,
            "verified process exit must terminate unanswered work even when its final native error is retryable");
    }
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .current_request
        .is_none());
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .provider_retry
        .is_none());
}

#[test]
fn removed_directory_textless_native_completion_closes_unanswered_work() {
    let (mut f, cwd, rollout, lead) = started();
    std::fs::remove_dir(&cwd).unwrap();
    f.worker.check_launch_directory(f.agent).unwrap().unwrap();
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout)
            .unwrap(),
        "{}",
        json!({"type":"event_msg","payload":{"type":"task_complete",
            "turn_id":"living-turn","last_agent_message":null}})
    )
    .unwrap();
    f.status(RuntimeStatus::Idle);
    f.now += TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(
        f.request(lead).phase,
        RequestPhase::Completed,
        "owned task_complete is a verified turn end even when Codex records no final text"
    );
    assert!(f.request(lead).failure_reason.is_none());
    assert!(f.reply(lead).is_none());
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .current_request
        .is_none());
}

#[test]
fn removed_directory_background_wait_closes_after_proved_process_exit() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let cwd = f.dir.join("background-launch-worktree");
    std::fs::create_dir(&cwd).unwrap();
    let mut state = f.worker.state.clone();
    state
        .update_agent_runtime_metadata(f.agent, cwd.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    let lead = running(&mut f, "wait for the background agent", "living-turn");
    f.record(
        json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"living-turn",
        "background_tasks":[{"type":"agent"}]}),
    );
    f.consume();
    assert!(f.request(lead).awaiting_background);
    std::fs::remove_dir(&cwd).unwrap();
    f.worker.check_launch_directory(f.agent).unwrap().unwrap();
    f.worker.settle_requests().unwrap();
    assert_eq!(
        f.request(lead).phase,
        RequestPhase::Active,
        "a living provider's background wait must stay open"
    );
    f.status(RuntimeStatus::Unavailable);
    let mut state = f.worker.state.clone();
    state
        .record_exit_snapshot(
            f.agent,
            ExitSnapshot {
                launch_id: Some("launch".into()),
                terminal_id: "terminal".into(),
                path: None,
                error: Some("verified native process exit".into()),
                tail: String::new(),
                at_ms: f.now,
                terminal_closed: Some(true),
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    f.now += TURN_SETTLE_MS + 1;
    f.status(RuntimeStatus::Unavailable);
    f.consume();
    assert_eq!(
        f.request(lead).phase,
        RequestPhase::Abandoned,
        "a proved process exit must terminate a background wait after launch-directory loss"
    );
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("agent_unavailable")
    );
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .current_request
        .is_none());
}
