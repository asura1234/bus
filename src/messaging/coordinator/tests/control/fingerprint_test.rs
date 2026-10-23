//! Dialog fingerprints expire an hour after the observation they vouch for.
use super::*;
use crate::messaging::model::AgentRuntimeIdentity;
use crate::messaging::storage::io::now_ms;

const MINUTE_MS: u64 = 60 * 1000;

fn launched_worker() -> (Worker, AgentId, std::path::PathBuf) {
    let dir = crate::utils::test_temp::unique_temp_path("bus-fingerprint");
    struct NoServer;
    impl crate::messaging::native::Transport for NoServer {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            panic!("an expired fingerprint must not reach the server: {method:?}")
        }
    }
    let mut worker = Worker::open(dir.clone(), Box::new(NoServer)).unwrap();
    let mut state = worker.state.clone();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            room,
            "builder",
            crate::messaging::model::Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: None,
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    (worker, agent, dir)
}

fn fingerprint(worker: &Worker, agent: AgentId, observed_at_ms: u64) -> String {
    let (room_id, launch_id, terminal_id, pane_id, session_id) =
        worker.dialog_identity(agent).unwrap();
    encode_dialog_fingerprint(&DialogFingerprintClaims {
        room_id,
        agent_id: agent,
        launch_id,
        terminal_id,
        pane_id,
        session_id,
        content_revision: 1,
        dialog_digest: "dialog-digest".into(),
        dialog_shape: "Do you want to proceed?".into(),
        options: 2,
        free_text: None,
        observed_at_ns: u128::from(observed_at_ms) * 1_000_000,
    })
    .unwrap()
}

#[test]
fn a_fingerprint_observed_over_an_hour_ago_is_refused_before_the_server_is_asked() {
    let (mut worker, agent, dir) = launched_worker();
    let stale = fingerprint(&worker, agent, now_ms() - 61 * MINUTE_MS);
    let error = worker.choose_dialog_option(agent, 1, &stale).unwrap_err();
    assert!(error.contains("expired"), "{error}");
    assert!(!worker.state.dialog_answer_spent(fingerprint_digest(&stale)));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn spent_fingerprints_are_forgotten_once_they_expire() {
    let (mut worker, _, dir) = launched_worker();
    let now = now_ms();
    let mut state = worker.state.clone();
    state.spend_dialog_answer("expired".into(), now - 61 * MINUTE_MS);
    state.spend_dialog_answer("recent".into(), now - MINUTE_MS);
    worker.save(state).unwrap();
    worker.archive_history(now).unwrap();
    assert!(!worker.state.dialog_answer_spent("expired"));
    assert!(worker.state.dialog_answer_spent("recent"));
    let saved = worker.store.load().unwrap().unwrap();
    assert!(!saved.dialog_answer_spent("expired"), "the drop is durable");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn version_one_spent_fingerprint_remains_single_use_after_upgrade() {
    let (mut worker, agent, dir) = launched_worker();
    let fresh = fingerprint(&worker, agent, now_ms());
    let mut state = serde_json::to_value(&worker.state).unwrap();
    let fields = state.as_object_mut().unwrap();
    fields.remove("spent_dialog_answers");
    fields.remove("consumed_callbacks");
    fields.insert(
        "consumed_dialog_fingerprints".into(),
        serde_json::json!([fresh]),
    );
    fields.insert("consumed_callback_ids".into(), serde_json::json!([]));
    std::fs::write(
        dir.join("state.json"),
        serde_json::to_vec(&serde_json::json!({"version": 1, "state": state})).unwrap(),
    )
    .unwrap();
    worker.state = worker.store.load().unwrap().unwrap();

    // The original observation is still fresh and has the same live launch;
    // retrying an already answered dialog must stop before any native keys.
    let error = worker.choose_dialog_option(agent, 1, &fresh).unwrap_err();
    assert!(error.contains("already used"), "{error}");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unavailable_dialog_snapshots_do_not_resolve_an_existing_help_notice() {
    struct RedrawServer;
    impl crate::messaging::native::Transport for RedrawServer {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentList(_) => Ok(ResponseResult::AgentList {
                    agents: vec![serde_json::from_value(serde_json::json!({
                        "terminal_id": "terminal", "pane_id": "pane", "name": "builder",
                        "agent": "claude", "workspace_id": "w", "tab_id": "t", "focused": false,
                        "interactive_ready": true, "revision": 1,
                        "agent_status": "idle", "dialog_id": null,
                    }))
                    .unwrap()],
                }),
                Method::AgentDialogObserve(_) => Err(TransportError {
                    code: Some("dialog_observation_unavailable".into()),
                    message: "Synchronized redraw has no coherent snapshot".into(),
                    definitely_rejected: true,
                }),
                other => panic!("unexpected request: {other:?}"),
            }
        }
    }
    let (mut worker, agent, dir) = launched_worker();
    worker.transport = Box::new(RedrawServer);
    let mut state = worker.state.clone();
    state
        .observe_dialog(agent, true, state.agent(agent).unwrap().observed_at_ms)
        .unwrap();
    state
        .set_dialog_notice(agent, Some("still-open-dialog".into()))
        .unwrap();
    worker.save(state).unwrap();

    // agent.list cannot parse a dialog while its snapshot is unavailable;
    // Independent screen detection can publish idle from partial redraw text.
    worker.poll().unwrap();
    worker.poll().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().dialog_notice.as_deref(),
        Some("still-open-dialog"),
        "an unreadable redraw cannot prove the dialog closed"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn captured_dialog_fingerprint_redrawing_is_a_definite_no_write() {
    struct Redrawing;
    impl crate::messaging::native::Transport for Redrawing {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            assert!(matches!(method, Method::AgentDialogChoose(_)));
            Err(TransportError {
                code: Some("dialog_redrawing".into()),
                message: "No keys were sent while the frame was incomplete".into(),
                definitely_rejected: true,
            })
        }
    }
    let (mut worker, agent, dir) = launched_worker();
    worker.transport = Box::new(Redrawing);
    let fresh = fingerprint(&worker, agent, now_ms());
    let error = worker.choose_dialog_option(agent, 1, &fresh).unwrap_err();
    assert!(
        error.starts_with("No keys were sent (dialog_redrawing)"),
        "{error}"
    );
    assert!(!error.contains("uncertain"));
    // The native rejection permits a fresh observation, not replaying the
    // already consumed fingerprint after a different dialog appears.
    let error = worker.choose_dialog_option(agent, 1, &fresh).unwrap_err();
    assert!(error.contains("already used"), "{error}");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
