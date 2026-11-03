use super::launch_metrics_tests::{ledger, seed_launch};
use super::*;
use crate::messaging::storage::state_store::SaveStage;

#[test]
fn launch_metrics_definite_start_rejection_keeps_its_exact_cause() {
    for code in [Some("command_failed"), None] {
        let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
        seed_launch(&mut worker, agent, u64::MAX);
        worker
            .watch_agent_startup(agent, vec!["codex".into()])
            .unwrap();
        let result = worker.record_failed_agent_start(
            agent,
            vec![],
            Err(TransportError {
                code: code.map(str::to_owned),
                message: "launch rejected".into(),
                definitely_rejected: true,
            }),
        );
        assert!(result.is_err());
        let saved = ledger(&worker);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(saved["1"]["outcome"], "failed");
        assert_eq!(saved["1"]["cause"], serde_json::to_value(code).unwrap());
    }
}
#[test]
fn launch_metrics_unknown_start_response_and_recoverable_shell_wait_stay_pending() {
    for (code, rejected) in [
        ("transport_timeout", false),
        ("agent_shell_waiting", true),
        ("agent_shell_starting", true),
    ] {
        let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![]);
        seed_launch(&mut worker, agent, u64::MAX);
        worker
            .watch_agent_startup(agent, vec!["codex".into()])
            .unwrap();
        let result = worker.record_failed_agent_start(
            agent,
            vec![],
            Err(TransportError {
                code: Some(code.into()),
                message: "requires inspection".into(),
                definitely_rejected: rejected,
            }),
        );
        assert!(result.is_err());
        let saved = ledger(&worker);
        let invoked = calls.lock().unwrap().clone();
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(saved["1"]["outcome"], "pending", "{code}");
        assert!(!invoked.iter().any(|m| m.starts_with("agent.start")));
    }
}
struct RejectTerminal {
    state_path: PathBuf,
    code: Option<&'static str>,
}
impl Transport for RejectTerminal {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(method, Method::TabCreate(_)) {
            let saved = JsonStore::new(self.state_path.clone())
                .load()
                .unwrap()
                .unwrap();
            let attempt = saved
                .launch_attempts()
                .next()
                .expect("attempt durable before terminal creation");
            assert_eq!(
                attempt.outcome,
                crate::messaging::model::LaunchOutcome::Pending
            );
            assert!(attempt.agent_id.is_some());
            assert!(attempt.launch_id.is_some());
            assert!(attempt.terminal_id.is_none());
            return Err(TransportError {
                code: self.code.map(str::to_owned),
                message: "terminal rejected".into(),
                definitely_rejected: true,
            });
        }
        Err(TransportError {
            code: None,
            message: "no diagnostic native screen".into(),
            definitely_rejected: false,
        })
    }
}
#[test]
fn launch_metrics_terminal_rejection_is_durable_and_retains_the_native_cause() {
    let _providers = provider_binaries();
    for code in [Some("unsupported_shell"), None] {
        let (mut worker, previous, room, dir, _) = fixture(Provider::Codex, vec![]);
        let mut state = worker.state.clone();
        state.delete_agent(previous, 0).unwrap();
        worker.save(state).unwrap();
        worker.transport = Box::new(RejectTerminal {
            state_path: dir.join("state.json"),
            code,
        });
        let result = worker.add_agent(
            AddAgent {
                room,
                name: "terminal-rejected".into(),
                provider: Provider::Codex,
                cwd: dir.to_string_lossy().into(),
                extra_args: String::new(),
                consent_project_hooks: true,
            },
            None,
            &mpsc::channel().0,
        );
        assert!(result.is_err());
        let saved = ledger(&worker);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(saved["1"]["outcome"], "failed");
        assert_eq!(
            saved["1"]["cause"],
            code.unwrap_or("terminal_creation_failed")
        );
    }
}
#[test]
fn launch_metrics_failed_initial_save_prevents_preparation_and_native_input() {
    let _providers = provider_binaries();
    let (mut worker, _, room, dir, calls) = fixture(Provider::Codex, vec![]);
    std::fs::remove_file(dir.join("state.json")).unwrap();
    std::fs::create_dir(dir.join("state.json")).unwrap();
    let capture = crate::utils::logging::test_capture::Capture::default();
    let mut outcome = None;
    capture.run(|| {
        outcome = Some(worker.add_agent(
            AddAgent {
                room,
                name: "no-save".into(),
                provider: Provider::Codex,
                cwd: dir.to_string_lossy().into(),
                extra_args: String::new(),
                consent_project_hooks: true,
            },
            None,
            &mpsc::channel().0,
        ))
    });
    let result = outcome.unwrap();
    assert!(
        !capture.text().contains("bus.agent.launch_outcome"),
        "{}",
        capture.text()
    );
    assert!(
        result.as_ref().is_err_and(|e| e.contains("Storage paused")),
        "{result:?}"
    );
    assert!(calls.lock().unwrap().is_empty());
    assert!(
        !dir.join(".codex").exists(),
        "preparation ran before the durable attempt save"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn launch_metrics_storage_recovery_logs_the_persisted_outcome() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, u64::MAX);
    let mut state = worker.state.clone();
    super::super::launch_metrics::settle_current(
        &mut state,
        agent,
        crate::messaging::model::LaunchOutcome::Failed,
        Some("command_failed"),
        2,
    );
    worker.store.fail_once_at(SaveStage::TempSync);
    assert!(worker.save(state.clone()).is_err());
    worker.state = state;
    assert_eq!(ledger(&worker)["1"]["outcome"], "failed");
    assert_eq!(
        serde_json::to_value(&worker.durable_state).unwrap()["launch_attempts"]["1"]["outcome"],
        "pending"
    );
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (events, _) = mpsc::channel();
        worker.retry_storage(
            std::time::Instant::now() + std::time::Duration::from_secs(2),
            &events,
        );
    });
    assert!(
        capture.text().contains("bus.agent.launch_outcome"),
        "{}",
        capture.text()
    );
    assert!(
        capture.text().contains("attempt_id=1"),
        "{}",
        capture.text()
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
