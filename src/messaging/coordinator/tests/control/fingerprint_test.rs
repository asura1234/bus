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
