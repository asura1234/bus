//! Callback-binding regressions from the unbound-callback investigation; no live provider.
use super::*;

fn master_fixture() -> (Worker, AgentId, RoomId, PathBuf) {
    let (mut worker, previous, work, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let mut state = worker.state.clone();
    let identity = state.agent(previous).unwrap().runtime_identity.clone();
    state.delete_agent(previous).unwrap();
    let master = state.master_room().unwrap().id;
    let agent = state
        .create_agent(master, "lead", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(agent, work).unwrap();
    state.set_agent_runtime_identity(agent, identity).unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    worker.save(state).unwrap();
    callbacks::initialize(
        &dir.join("callbacks/launch"),
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(agent.0),
            provider: Provider::ClaudeCode,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    (worker, agent, master, dir)
}

#[test]
fn same_turn_claude_input_is_progress_without_exposing_content() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir) = master_fixture();
        let request = queue(&mut worker, room, agent, "PRIVATE_ORIGINAL");
        worker.submit_ready().unwrap();
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("turn", "PRIVATE_ORIGINAL"),
        );
        consume(&mut worker, agent, &dir);
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("turn", "PRIVATE_OTHER_INPUT"),
        );
        consume(&mut worker, agent, &dir);
        let logs = capture.text();
        assert!(logs.contains("disposition=AcceptedProgress"), "{logs}");
        assert_eq!(
            worker.state.agent(agent).unwrap().current_request,
            Some(request)
        );
        assert!(worker.state.request(request).unwrap().trusted_start_bound);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        assert!(!logs.contains("Rejected("), "{logs}");
        let progress = logs
            .lines()
            .find(|line| line.contains("disposition=AcceptedProgress"))
            .unwrap();
        for field in [
            "bus.callback.binding",
            "trusted_start_bound=true",
            "expected_session_id=Some(\"session\")",
            "callback_session_id=Some(\"session\")",
            "expected_turn_id=Some(\"turn\")",
            "callback_turn_id=Some(\"turn\")",
            "expected_prompt_id=Some(\"turn\")",
            "callback_prompt_id=Some(\"turn\")",
            "raw_payload_matches=Some(false)",
            "submission_boundary=Some(0)",
        ] {
            assert!(progress.contains(field), "missing {field}: {progress}");
        }
        assert!(!logs.contains("PRIVATE_ORIGINAL") && !logs.contains("PRIVATE_OTHER_INPUT"));
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("turn", "PRIVATE_REPLY", json!([])),
        );
        consume(&mut worker, agent, &dir);
        // More input in the same owned turn must not clear a captured answer,
        // even after restoring the durable request binding.
        drop(worker);
        let mut worker = reopen_saved(&dir);
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("turn", "PRIVATE_LATE_INPUT"),
        );
        consume(&mut worker, agent, &dir);
        assert_eq!(
            worker
                .state
                .request(request)
                .unwrap()
                .pending_final
                .as_ref()
                .unwrap()
                .text,
            "PRIVATE_REPLY"
        );
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
            .unwrap();
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Completed
        );
        assert_eq!(
            worker.state.room(room).unwrap().latest_replies[&agent].text,
            "PRIVATE_REPLY"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn claude_compaction_and_delayed_submit_preserve_request_binding() {
    for compact_before_start in [true, false] {
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            let (mut worker, agent, room, dir) = master_fixture();
            let request = queue(&mut worker, room, agent, "owned prompt");
            worker.submit_ready().unwrap();
            if !compact_before_start {
                record(&dir, Provider::ClaudeCode, claude_start("turn", "owned prompt"));
                consume(&mut worker, agent, &dir);
            }
            for _ in 0..3 {
                record(&dir, Provider::ClaudeCode, json!({"hook_event_name":"SessionStart", "session_id":"session", "source":"compact"}));
                consume(&mut worker, agent, &dir);
            }
            record(&dir, Provider::ClaudeCode, claude_stop("turn", "reply", json!([])));
            consume(&mut worker, agent, &dir);
            if compact_before_start {
                assert_eq!(worker.state.agent(agent).unwrap().current_request, Some(request));
                assert_eq!(callbacks::records(&dir.join("callbacks/launch")).unwrap().len(), 1);
                // Recover a delayed hook after reopening the persisted scratch state.
                drop(worker);
                worker = reopen_saved(&dir);
                record(&dir, Provider::ClaudeCode, claude_start("turn", "owned prompt"));
                consume(&mut worker, agent, &dir);
            }
            worker.state.observe_status(agent, RuntimeStatus::Idle, io::now_ms()).unwrap();
            assert_eq!(worker.state.request(request).unwrap().phase, RequestPhase::Completed);
            assert_eq!(worker.state.room(room).unwrap().latest_replies[&agent].text, "reply");
            assert!(worker.state.agent(agent).unwrap().actionable_error.is_none());
            assert_eq!(worker.state.agent(agent).unwrap().compactions.count, 3);
            assert!(callbacks::records(&dir.join("callbacks/launch")).unwrap().is_empty());
            drop(worker);
            std::fs::remove_dir_all(dir).unwrap();
        });
    }
}

#[test]
fn callback_binding_diagnostics_explain_missing_submit() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir) = master_fixture();
        let request = queue(&mut worker, room, agent, "PRIVATE_PROMPT");
        worker.submit_ready().unwrap();
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("turn", "PRIVATE_REPLY", json!([])),
        );
        consume(&mut worker, agent, &dir);
        assert_eq!(
            worker.state.agent(agent).unwrap().current_request,
            Some(request)
        );
        assert!(!worker.state.request(request).unwrap().trusted_start_bound);
        let logs = capture.text();
        let deferred = logs
            .lines()
            .find(|line| line.contains("trusted_start_missing"))
            .unwrap();
        for field in [
            "bus.callback.binding",
            "trusted_start_bound=false",
            "expected_turn_id=None",
            "callback_turn_id=Some(\"turn\")",
            "compactions=0",
        ] {
            assert!(deferred.contains(field), "missing {field}: {deferred}");
        }
        assert!(!logs.contains("PRIVATE_PROMPT") && !logs.contains("PRIVATE_REPLY"));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn changed_claude_session_preserves_the_active_request_and_logs_both_sessions() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir) = master_fixture();
        let request = queue(&mut worker, room, agent, "owned prompt");
        worker.submit_ready().unwrap();
        record(&dir, Provider::ClaudeCode, claude_start("turn", "owned prompt"));
        consume(&mut worker, agent, &dir);
        record(&dir, Provider::ClaudeCode, json!({"hook_event_name":"SessionStart", "session_id":"changed-session", "source":"compact"}));
        consume(&mut worker, agent, &dir);
        let owner = worker.state.agent(agent).unwrap();
        assert_eq!(owner.current_request, Some(request));
        assert_eq!(owner.runtime_identity.session_id.as_deref(), Some("session"));
        assert!(owner.session_binding_invalidated);
        assert_eq!(worker.state.request(request).unwrap().provider_session_id.as_deref(), Some("session"));
        let logs = capture.text();
        let mismatch = logs.lines().find(|line| line.contains("session_mismatch")).unwrap();
        assert!(mismatch.contains("expected_session_id=Some(\"session\")"), "{mismatch}");
        assert!(mismatch.contains("callback_session_id=changed-session"), "{mismatch}");
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}
