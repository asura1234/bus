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

// These are coordinator contract tests, not subprocess startup benchmarks.
// Leave headroom for the stand-in interpreter under instrumented gate load;
// production timeouts remain unchanged.
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(45);

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
    _receiver: mpsc::Receiver<BusEvent>,
    agent: AgentId,
    now: u64,
}
impl Fixture {
    fn new() -> Self {
        let root = crate::utils::test_temp::unique_temp_path("feishu-waiting");
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("lark-cli");
        std::fs::write(&script, include_str!("waiting_cli_fixture.txt")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut worker = Worker::open(root.join("session"), Box::new(NoNative)).unwrap();
        worker.settings_path = Some(root.join("settings.json"));
        settings::save(
            worker.settings_path.as_ref().unwrap(),
            &settings::BusSettings::default(),
        )
        .unwrap();
        worker.attention.client = Client::on_path(&root, CLI_TIMEOUT);
        let mut state = worker.state.clone();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let agent = state
            .create_agent(master, "orch", Provider::ClaudeCode, root.clone(), None)
            .unwrap();
        state.bind_orchestrator(agent, work).unwrap();
        worker.save(state).unwrap();
        let (events, receiver) = mpsc::channel();
        let mut fixture = Self {
            worker,
            root,
            events,
            _receiver: receiver,
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
        let master = state.master_room().unwrap().id;
        state
            .post_to_developer(master, self.agent, text.into(), vec![], self.now)
            .unwrap();
        self.worker.save(state).unwrap();
        self.pump();
    }
    fn flush(&mut self) {
        self.now += 2000;
        self.pump();
        let deadline = Instant::now() + COMPLETION_TIMEOUT;
        while self.worker.attention.job.is_some() {
            assert!(Instant::now() < deadline, "send did not complete");
            std::thread::sleep(Duration::from_millis(5));
            self.pump();
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
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn feishu_waiting_variants_invoke_bot_dm_once_with_exact_action_and_argv() {
    for marker in ["[waiting-on-you]", "[waiting_on_you]", "[WAITING_ON_YOU]"] {
        let mut f = Fixture::new();
        f.post(&format!(
            "Summary\n{marker}\nApprove access\nPRIVATE attachment /tmp/secret"
        ));
        f.flush();
        assert_eq!(f.worker.attention.status.error, None, "{marker}");
        f.now += 5000;
        f.pump();
        let calls = f.calls();
        assert_eq!(
            calls.len(),
            1,
            "{marker}; status={:?}; config={:?}; waiting={:?}; notices={:?}",
            f.worker.attention.status,
            f.worker.attention.config,
            f.worker.attention.waiting,
            f.worker
                .state
                .notices(f.worker.state.master_room().unwrap().id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            &calls[0][..8],
            [
                "im",
                "+messages-send",
                "--as",
                "bot",
                "--user-id",
                settings::DEFAULT_FEISHU_RECIPIENT,
                "--text",
                "orch is waiting on you: Approve access"
            ]
        );
        assert_eq!(&calls[0][8..9], ["--idempotency-key"]);
        assert_eq!(&calls[0][10..], ["--format", "json"]);
    }
}

#[test]
fn feishu_ignores_untagged_posts_and_the_legacy_phrase() {
    // Status replies such as "nothing is waiting on you" must never page the developer.
    for text in [
        "Merged #179; nothing is waiting on you.",
        "still waiting on you: Approve access",
        "[needs_developer_attention] Approve access",
        "Decide release timing",
    ] {
        let mut f = Fixture::new();
        f.post(text);
        f.flush();
        f.now += 5000;
        f.pump();
        assert!(f.calls().is_empty(), "{text}");
    }
}

#[test]
fn feishu_full_unicode_action_reaches_cli_without_truncation() {
    let mut f = Fixture::new();
    let action = "界 full action ".repeat(100);
    f.post(&format!("[waiting_on_you] {action}\nPRIVATE"));
    f.flush();
    assert_eq!(f.calls().len(), 1);
    assert_eq!(
        f.calls()[0][7],
        format!("orch is waiting on you: {}", action.trim())
    );
}

#[test]
fn feishu_direct_messages_notify_and_legacy_off_cannot_silence_or_replay() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Approve access");
    f.flush();
    assert_eq!(f.calls().len(), 1);
    assert_eq!(f.calls()[0][7], "orch is waiting on you: Approve access");
    f.settings(|s| s.feishu_notifications = false);
    f.post("[waiting_on_you] mandatory");
    f.flush();
    assert_eq!(f.calls().len(), 2);
    f.settings(|s| s.feishu_notifications = true);
    f.flush();
    assert_eq!(f.calls().len(), 2);
}

#[test]
fn feishu_bursts_coalesce_and_disable_is_rejected_without_losing_pending() {
    let mut f = Fixture::new();
    f.post("[waiting_on_you] first");
    f.now += 1900;
    f.post("[waiting-on-you] second");
    assert!(f.calls().is_empty());
    f.now += 100;
    f.pump();
    f.flush();
    assert_eq!(f.calls().len(), 1);
    assert_eq!(
        f.calls()[0][7],
        "orch is waiting on you: first\norch is waiting on you: second"
    );
    f.post("[waiting_on_you] cancelled");
    assert!(f.worker.set_feishu_notifications(false, &f.events).is_err());
    f.flush();
    assert_eq!(f.calls().len(), 2);
}

#[test]
fn feishu_failure_logs_warning_and_never_blocks_coordinator_or_retries() {
    let mut f = Fixture::new();
    std::fs::write(f.root.join("mode"), "fail").unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    #[derive(Clone)]
    struct Writer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let writer = Writer(log.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        f.post("[waiting_on_you] action");
        f.flush();
        for _ in 0..5 {
            f.now += 10_000;
            f.pump();
        }
    });
    let log = String::from_utf8(log.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("WARN")
            && log.contains("bus.feishu.failed")
            && log.contains("exit code Some(7)"),
        "{log}"
    );
    assert_eq!(f.calls().len(), 1);
    std::fs::write(f.root.join("mode"), "hang").unwrap();
    // The stand-in sleeps for 60 seconds, so this still exercises a real
    // timeout without requiring the interpreter to start within one second.
    f.worker.attention.client = Client::on_path(&f.root, Duration::from_secs(10));
    f.post("[waiting_on_you] timed out");
    f.now += 2000;
    let start = Instant::now();
    f.pump();
    assert!(start.elapsed() < Duration::from_millis(500));
    f.flush();
    assert!(f
        .worker
        .attention
        .status
        .error
        .as_deref()
        .unwrap()
        .contains("timed out"));
    assert_eq!(f.calls().len(), 2);
}

#[test]
fn feishu_missing_cli_and_configured_destination_and_override_are_bounded() {
    let mut f = Fixture::new();
    let program = f.root.join("lark-cli").to_string_lossy().into_owned();
    f.settings(|s| {
        s.feishu_recipient = "ou_configured".into();
        s.feishu_cli_path = Some(program);
    });
    f.post("[waiting_on_you] configured");
    f.flush();
    assert_eq!(f.calls()[0][5], "ou_configured");
    f.settings(|s| s.feishu_cli_path = Some("/missing/lark-cli".into()));
    f.post("[waiting_on_you] missing");
    f.flush();
    assert!(f
        .worker
        .attention
        .status
        .error
        .as_deref()
        .unwrap()
        .contains("not installed"));
    assert_eq!(f.calls().len(), 1);
}

#[test]
fn feishu_startup_history_and_destination_changes_do_not_replay() {
    let mut f = Fixture::new();
    f.post("[waiting_on_you] before restart");
    let client = f.worker.attention.client.clone();
    f.worker.attention = Runtime::default();
    f.worker.attention.client = client;
    f.pump();
    f.flush();
    assert!(f.calls().is_empty());
    f.post("[waiting_on_you] old destination");
    f.settings(|s| s.feishu_recipient = "ou_new".into());
    f.flush();
    assert_eq!(f.calls().len(), 1);
    assert_eq!(f.calls()[0][5], "ou_new");
    assert_eq!(f.calls()[0][7], "orch is waiting on you: old destination");
    f.post("[waiting_on_you] new action");
    f.flush();
    assert_eq!(f.calls().len(), 2);
    assert_eq!(f.calls()[1][5], "ou_new");
}

#[test]
fn feishu_upgrade_starts_mandatory_dms_and_ignores_legacy_off_choice() {
    // Every settings.json written before waiting DMs serialized the old
    // default-off `feishu_notifications` and the removed popup toggle.
    let mut f = Fixture::new();
    std::fs::write(
        f.worker.settings_path.as_ref().unwrap(),
        r#"{
  "color_blind_mode": false,
  "feishu_notifications": false,
  "developer_notifications": true,
  "feishu_notifications_generation": 0,
  "master_sound": { "enabled": true, "name": null },
  "room_sound": { "enabled": false, "name": null },
  "max_compactions_per_agent": 5,
  "background_shell_hold_minutes": 15,
  "sidebar_width": 28
}"#,
    )
    .unwrap();
    f.pump();
    f.post("[waiting_on_you] Approve access");
    f.flush();
    assert_eq!(
        f.calls().len(),
        1,
        "waiting DM stayed off after upgrade; enabled={}",
        f.worker.attention.status.enabled
    );
    f.settings(|s| s.feishu_notifications = false);
    f.post("[waiting_on_you] Disabled after upgrade");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "legacy off cannot disable developer alerts"
    );
}

#[test]
fn notify_you_legacy_off_cannot_disable_worker_notifications() {
    let mut f = Fixture::new();
    f.settings(|s| s.feishu_notifications = false);
    let mut state = f.worker.state.clone();
    let room = state
        .rooms()
        .find(|r| r.kind == crate::messaging::model::RoomKind::Work)
        .unwrap()
        .id;
    let sender = state
        .create_agent(room, "worker", Provider::Codex, f.root.clone(), None)
        .unwrap();
    state
        .post_to_developer(
            room,
            sender,
            "[waiting-on-you] Decide release timing".into(),
            vec![],
            f.now,
        )
        .unwrap();
    f.worker.save(state).unwrap();
    f.pump();
    f.flush();
    assert_eq!(f.calls().len(), 1);
    assert_eq!(
        f.calls()[0][7],
        "worker is waiting on you: Decide release timing"
    );
    assert!(f.worker.set_feishu_notifications(false, &f.events).is_err());
}

#[test]
fn repeated_tagged_blocker_after_failed_delivery_can_be_sent_after_recovery() {
    let mut f = Fixture::new();
    std::fs::write(f.root.join("mode"), "fail").unwrap();
    f.post("[waiting-on-you] Unlock the device");
    f.flush();
    assert!(f.worker.attention.status.error.is_some());
    assert_eq!(f.calls().len(), 1);
    std::fs::remove_file(f.root.join("mode")).unwrap();
    f.now += 5_000;
    f.post("[waiting-on-you] Unlock the device");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "a failed attempt must not suppress a fresh explicit blocker after delivery recovers"
    );
    assert_eq!(f.worker.attention.status.error, None);
}

#[test]
fn repeated_tagged_ask_stays_deduplicated_after_sender_rename() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Unlock the device");
    f.flush();
    assert_eq!(f.calls().len(), 1);
    let mut state = f.worker.state.clone();
    state.rename_agent(f.agent, "renamed-orch").unwrap();
    f.worker.save(state).unwrap();
    f.pump();
    f.now += 5_000;
    f.post("[waiting-on-you] Unlock the device");
    f.flush();
    assert_eq!(
        f.calls().len(),
        1,
        "renaming does not change the sender ID of an already paged ask"
    );
}

#[test]
fn distinct_sender_reusing_a_name_can_page_its_own_tagged_ask() {
    let mut f = Fixture::new();
    f.post("[waiting-on-you] Unlock the device");
    f.flush();
    assert_eq!(f.calls().len(), 1);
    let mut state = f.worker.state.clone();
    state.rename_agent(f.agent, "renamed-orch").unwrap();
    let master = state.master_room().unwrap().id;
    let replacement = state
        .create_agent(master, "orch", Provider::Codex, f.root.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    f.agent = replacement;
    f.pump();
    f.now += 5_000;
    f.post("[waiting-on-you] Unlock the device");
    f.flush();
    assert_eq!(
        f.calls().len(),
        2,
        "a different sender ID must not inherit another sender's dedupe entry"
    );
}
