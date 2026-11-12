use super::*;
use crate::messaging::coordinator::{Method, ResponseResult};
use crate::messaging::{
    model::{AgentId, Provider},
    native::{Transport, TransportError},
};
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

struct NoNative;
impl Transport for NoNative {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        panic!("notifications must not drive native terminals");
    }
}

struct Fixture {
    worker: Worker,
    root: PathBuf,
    events: mpsc::Sender<BusEvent>,
    receiver: mpsc::Receiver<BusEvent>,
    agent: AgentId,
    now: u64,
}
impl Fixture {
    fn new() -> Self {
        let root = crate::utils::test_temp::unique_temp_path("feishu-waiting");
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("feishu-bridge");
        std::fs::write(&script, include_str!("waiting_cli_fixture.txt")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut worker = Worker::open(root.join("session"), Box::new(NoNative)).unwrap();
        worker.settings_path = Some(root.join("settings.json"));
        settings::save(
            worker.settings_path.as_ref().unwrap(),
            &settings::BusSettings {
                feishu_bridge_path: Some(script.to_str().unwrap().into()),
                ..Default::default()
            },
        )
        .unwrap();
        worker.attention.client = Client::on_path(&root, Duration::from_secs(10));
        let mut state = worker.state.clone();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let agent = state
            .create_agent(master, "orch", Provider::Codex, root.clone(), None)
            .unwrap();
        state.bind_orchestrator(agent, work).unwrap();
        worker.save(state).unwrap();
        let (events, receiver) = mpsc::channel();
        let mut fixture = Self {
            worker,
            root,
            events,
            receiver,
            agent,
            now: 1000,
        };
        fixture.pump();
        fixture
    }
    fn pump(&mut self) {
        self.worker.pump_feishu(&self.events, self.now).unwrap();
    }
    fn post(&mut self, text: &str) {
        let mut state = self.worker.state.clone();
        state
            .post_to_developer(
                state.master_room().unwrap().id,
                self.agent,
                text.into(),
                vec![],
                self.now,
            )
            .unwrap();
        self.worker.save(state).unwrap();
        self.pump();
    }
    fn flush(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.pump();
            if self.worker.attention.job.is_none()
                && self.worker.attention.completion.is_none()
                && self.worker.attention.status.queued == 0
            {
                break;
            }
            assert!(Instant::now() < deadline, "send did not complete");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn calls(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.root.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn settings(&mut self, change: impl FnOnce(&mut settings::BusSettings)) {
        settings::update(self.worker.settings_path.as_ref().unwrap(), change).unwrap();
        self.pump();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.worker.attention.stop();
        let retired = std::mem::take(&mut self.worker.attention.retired);
        for job in retired {
            let _ = job.join();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn feishu_each_request_has_its_own_dm_and_durable_mapping_before_and_after_transport() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Choose");
    f.post("[waiting-on-you] Choose");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "same text from distinct requests must retain both routes"
    );
    assert_eq!(f.worker.state.feishu.outbox.len(), 2);
    let saved = f.worker.store.load().unwrap().unwrap();
    for record in saved.feishu.outbox.values() {
        assert!(record.message_id.as_ref().unwrap().starts_with("om_fake_"));
        assert_eq!(record.chat_id.as_deref(), Some("oc_dm"));
        assert_eq!(record.orchestrator, Some(f.agent));
        assert_eq!(record.raising_agent, Some(f.agent));
        assert!(record.text.contains(&format!("bus:{}", record.code)));
    }
    assert_ne!(f.calls()[0][6], f.calls()[1][6]);
    assert_eq!(f.worker.attention.status.error, None);
}

#[test]
fn feishu_missing_plugin_turns_both_transport_directions_off_without_a_tui_error() {
    let mut f = Fixture::new();
    f.settings(|s| s.feishu_bridge_path = Some("/missing/feishu-bridge".into()));
    f.post("[waiting-on-you] Choose");
    assert!(!f.worker.attention.status.enabled);
    assert!(f.calls().is_empty());
    assert!(f.worker.state.feishu.outbox.is_empty());
    assert_eq!(f.worker.attention.status.error, None);
    assert!(!f
        .receiver
        .try_iter()
        .any(|event| matches!(event, BusEvent::CommandFinished { result: Err(_), .. })));
}

#[test]
fn feishu_untagged_posts_and_legacy_phrases_do_not_send() {
    let mut f = Fixture::new();
    for text in [
        "@you Choose",
        "still waiting on you: Choose",
        "Nothing is waiting on you",
        "Choose",
    ] {
        f.post(text);
    }
    f.flush();
    assert!(f.calls().is_empty());
}

#[test]
fn feishu_complete_unicode_action_and_literal_shell_text_reach_the_bridge() {
    let mut f = Fixture::new();
    let action = format!("{}\nNext: `$HOME` 开通权限", "界".repeat(100));
    f.post(&format!("[waiting-on-you] {action}"));
    f.flush();
    assert!(f.calls()[0][4].starts_with(&format!("orch is waiting on you: {}", action.trim())));
}

#[test]
fn feishu_attempted_send_is_never_replayed_after_failure_or_runtime_restart() {
    let mut f = Fixture::new();
    std::fs::write(f.root.join("mode"), "fail").unwrap();
    f.post("[waiting-on-you] Choose");
    f.flush();
    assert!(f.worker.attention.status.error.is_some());
    let client = f.worker.attention.client.clone();
    f.worker.attention = Runtime::default();
    f.worker.attention.client = client;
    f.pump();
    f.flush();
    assert_eq!(f.calls().len(), 1);
    std::fs::remove_file(f.root.join("mode")).unwrap();
    f.post("[waiting-on-you] Choose");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "a new explicit request may send after recovery"
    );
}

#[test]
fn feishu_bridge_failure_stays_in_logs_even_with_error_reporting_enabled() {
    use tracing_subscriber::prelude::*;
    let mut f = Fixture::new();
    f.worker.dev_enabled = true;
    f.worker
        .set_error_orchestrator(Some("orch".into()), &f.events)
        .unwrap();
    f.worker.refresh_error_pipe_at(100);
    let log = f.worker.error_log_dir().join("server.log");
    let subscriber =
        tracing_subscriber::registry().with(crate::utils::error_pipe::CaptureLayer::new(log));
    std::fs::write(f.root.join("mode"), "fail").unwrap();
    tracing::subscriber::with_default(subscriber, || {
        f.post("[waiting-on-you] Choose");
        f.flush();
    });
    f.worker.refresh_error_pipe_at(10_000);
    f.worker.refresh_error_pipe_at(20_000);
    assert!(f.worker.attention.status.error.is_some());
    assert_eq!(
        f.worker.state.requests().count(),
        0,
        "Feishu failure must not enqueue an error report"
    );
    assert!(!f
        .receiver
        .try_iter()
        .any(|event| matches!(event, BusEvent::CommandFinished { result: Err(_), .. })));
}

#[test]
fn feishu_new_runtime_uses_history_as_baseline_but_keeps_persisted_pending_outbox() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    state
        .post_to_developer(
            state.master_room().unwrap().id,
            f.agent,
            "[waiting-on-you] Old history".into(),
            vec![],
            1,
        )
        .unwrap();
    f.worker.save(state).unwrap();
    let client = f.worker.attention.client.clone();
    f.worker.attention = Runtime::default();
    f.worker.attention.client = client;
    f.pump();
    f.flush();
    assert!(f.calls().is_empty());
    f.post("[waiting-on-you] Fresh request");
    f.flush();
    assert_eq!(f.calls().len(), 1);
}
