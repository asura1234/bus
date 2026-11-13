use super::*;
use crate::messaging::coordinator::{Method, ResponseResult};
use crate::messaging::{
    model::{AgentId, PromptId, Provider},
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
    fn post(&mut self, text: &str) -> PromptId {
        let mut state = self.worker.state.clone();
        let source = state
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
        source
    }
    fn flush(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.pump();
            if self.worker.attention.job.is_none()
                && self.worker.attention.retired.is_empty()
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
fn feishu_distinct_asks_have_their_own_dm_and_durable_mapping_before_and_after_transport() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Choose first");
    f.post("[waiting-on-you] Choose second");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "distinct asks must retain separate DMs and routes"
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
fn feishu_missing_plugin_turns_notifications_off_without_a_tui_error() {
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
}

#[test]
fn repeated_tagged_blocker_after_failed_delivery_can_be_sent_after_recovery() {
    let mut f = Fixture::new();
    std::fs::write(f.root.join("mode"), "fail").unwrap();
    f.post("[waiting-on-you] Choose");
    f.flush();
    std::fs::remove_file(f.root.join("mode")).unwrap();
    f.post("[waiting-on-you] Choose");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "a repeated explicit ask must reach Feishu after recovery"
    );
    let saved = f.worker.store.load().unwrap().unwrap();
    assert_eq!(saved.feishu.outbox.len(), 2);
    assert_eq!(
        saved
            .feishu
            .outbox
            .values()
            .filter(|r| r.message_id.is_some())
            .count(),
        1
    );
}

#[test]
fn feishu_repeat_ask_after_an_uncertain_send_survives_restart_as_a_new_root() {
    let mut f = Fixture::new();
    std::fs::write(f.root.join("mode"), "hang").unwrap();
    let first = f.post("[waiting-on-you] Choose");
    let deadline = Instant::now() + Duration::from_secs(10);
    while f.calls().is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    f.worker.finish_feishu();
    std::fs::remove_file(f.root.join("mode")).unwrap();
    let replacement = Worker::open(f.root.join("unused-uncertain"), Box::new(NoNative)).unwrap();
    drop(std::mem::replace(&mut f.worker, replacement));
    f.worker = Worker::open(f.root.join("session"), Box::new(NoNative)).unwrap();
    f.worker.settings_path = Some(f.root.join("settings.json"));
    f.worker.attention.client = Client::on_path(&f.root, Duration::from_secs(10));
    f.now += 60_000;
    f.pump();
    f.post("[waiting-on-you] Choose");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "an uncertain send cannot suppress a repeated ask"
    );
    let saved = f.worker.store.load().unwrap().unwrap();
    let old = saved
        .feishu
        .outbox
        .values()
        .find(|r| r.source.prompt_id == first)
        .unwrap();
    assert!(old.attempted && old.message_id.is_none());
    assert!(serde_json::to_value(old).unwrap()["send_error"].is_string());
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

#[test]
fn feishu_successful_send_mapping_survives_unreadable_settings() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Choose");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f.worker.attention.job.as_ref().unwrap().is_finished() {
        assert!(Instant::now() < deadline, "bridge did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(f.calls().len(), 1);
    let settings_path = f.worker.settings_path.as_ref().unwrap().clone();
    let saved_settings = std::fs::read(&settings_path).unwrap();
    std::fs::write(&settings_path, b"{malformed").unwrap();
    f.pump();
    std::fs::write(&settings_path, saved_settings).unwrap();
    f.flush();
    assert_eq!(f.calls().len(), 1, "settings recovery must not resend");
    let saved = f.worker.store.load().unwrap().unwrap();
    let record = saved.feishu.outbox.values().next().unwrap();
    assert_eq!(
        record.message_id.as_deref(),
        Some("om_fake_1"),
        "a successful bridge completion must survive a settings read failure"
    );
    assert_eq!(record.chat_id.as_deref(), Some("oc_dm"));
}

#[test]
fn feishu_successful_send_mapping_survives_worker_retirement() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Choose");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f.worker.attention.job.as_ref().unwrap().is_finished() {
        assert!(Instant::now() < deadline, "bridge did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    // A settings failure can retire the worker just after its finished check.
    f.worker.attention.stop();
    f.flush();
    assert_eq!(f.calls().len(), 1);
    let saved = f.worker.store.load().unwrap().unwrap();
    let record = saved.feishu.outbox.values().next().unwrap();
    assert_eq!(record.message_id.as_deref(), Some("om_fake_1"));
}

#[test]
fn feishu_successful_send_mapping_survives_worker_shutdown() {
    for explicit in [true, false] {
        let mut f = Fixture::new();
        f.post("[waiting-on-you] Choose");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !f.worker.attention.job.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline, "bridge did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        let replacement = Worker::open(f.root.join("unused"), Box::new(NoNative)).unwrap();
        let worker = std::mem::replace(&mut f.worker, replacement);
        let snapshots = std::sync::Arc::new(std::sync::Mutex::new(std::sync::Arc::new(
            worker.snapshot(),
        )));
        let (commands, receiver) = mpsc::channel();
        if explicit {
            commands
                .send((1, crate::messaging::coordinator::BusCommand::Shutdown))
                .unwrap();
        }
        drop(commands);
        worker.run(receiver, f.events.clone(), snapshots);
        let saved = crate::messaging::storage::state_store::JsonStore::new(
            f.root.join("session/state.json"),
        )
        .load()
        .unwrap()
        .unwrap();
        assert_eq!(f.calls().len(), 1);
        assert_eq!(
            saved
                .feishu
                .outbox
                .values()
                .next()
                .unwrap()
                .message_id
                .as_deref(),
            Some("om_fake_1")
        );
    }
}

#[test]
fn feishu_repeat_ask_keeps_one_dm_and_both_persisted_request_routes() {
    let mut f = Fixture::new();
    let first = f.post("[waiting-on-you] Restart Bus");
    f.flush();
    f.now += 60_000;
    let repeated = f.post("[waiting_on_you] Restart Bus");
    f.flush();
    assert_eq!(f.calls().len(), 1, "one shared DM within thirty minutes");
    let mut saved = f.worker.store.load().unwrap().unwrap();
    assert_eq!(saved.feishu.outbox.len(), 1);
    let record = serde_json::to_value(saved.feishu.outbox.values().next().unwrap()).unwrap();
    assert_eq!(record["source"]["prompt_id"], serde_json::json!(first));
    assert_eq!(
        record["aliases"][0]["source"]["prompt_id"],
        serde_json::json!(repeated)
    );
    assert_eq!(record["message_id"], "om_fake_1");
    assert_eq!(
        record["aliases"][0]["raising_agent"],
        serde_json::json!(f.agent)
    );
    // Re-observing retained history cannot append a logical source twice.
    saved.prepare_feishu_alerts(
        &BusState::new(),
        &f.worker.data_dir.to_string_lossy(),
        &settings::BusSettings::default().feishu_recipient,
        f.now,
    );
    assert_eq!(saved.feishu.outbox.len(), 1);
    assert_eq!(
        saved.feishu.outbox.values().next().unwrap().aliases.len(),
        1
    );
}

#[test]
fn feishu_repeat_ask_window_survives_restart_and_is_not_extended_by_a_repeat() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    let replacement = Worker::open(f.root.join("unused-restart"), Box::new(NoNative)).unwrap();
    drop(std::mem::replace(&mut f.worker, replacement));
    f.worker = Worker::open(f.root.join("session"), Box::new(NoNative)).unwrap();
    f.worker.settings_path = Some(f.root.join("settings.json"));
    f.worker.attention.client = Client::on_path(&f.root, Duration::from_secs(10));
    f.now += 60_000;
    f.pump();
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    assert_eq!(f.calls().len(), 1, "restart must retain suppression");
    f.now = 1_000 + 30 * 60_000;
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    assert_eq!(f.calls().len(), 2, "the window starts at the first request");
}

#[test]
fn feishu_repeat_ask_does_not_reuse_a_dm_for_another_recipient() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    f.settings(|settings| settings.feishu_recipient = "ou_other_recipient".into());
    f.now += 60_000;
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    assert_eq!(f.calls().len(), 2);
    assert_eq!(f.calls()[1][2], "ou_other_recipient");
}

#[test]
fn feishu_repeat_ask_keeps_different_raising_agents_separate() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    let mut state = f.worker.state.clone();
    f.agent = state
        .create_agent(
            state.master_room().unwrap().id,
            "other",
            Provider::Codex,
            f.root.clone(),
            None,
        )
        .unwrap();
    f.worker.save(state).unwrap();
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    assert_eq!(f.calls().len(), 2);
}

#[test]
fn feishu_repeat_ask_does_not_guess_legacy_ask_identity() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    let mut saved = serde_json::to_value(&f.worker.state).unwrap();
    for record in saved["feishu"]["outbox"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        record.as_object_mut().unwrap().remove("ask_body");
        record.as_object_mut().unwrap().remove("aliases");
    }
    f.worker
        .save(serde_json::from_value(saved).unwrap())
        .unwrap();
    f.post("[waiting-on-you] Restart Bus");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "legacy rows have no reliable normalized ask"
    );
}
