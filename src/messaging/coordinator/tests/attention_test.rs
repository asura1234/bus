use super::*;
use crate::messaging::control::{Request as ControlRequest, Response};
use crate::messaging::coordinator::{AgentId, Method, Provider, ResponseResult, RoomId};
use crate::messaging::native::{Transport, TransportError};
use crate::messaging::prefs::settings;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct NoNative;
impl Transport for NoNative {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        panic!("notification settings/delivery do not drive native terminals");
    }
}

struct Fixture {
    worker: Option<Worker>,
    root: PathBuf,
    events: mpsc::Sender<BusEvent>,
    _receiver: mpsc::Receiver<BusEvent>,
    lease: std::fs::File,
    next_call: u64,
    agent: AgentId,
    room: RoomId,
}

fn install_cli(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let program = root.join("lark-cli");
    std::fs::write(
        &program,
        include_str!("../../feishu/tests/lark_fixture.txt"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    program
}

impl Fixture {
    fn new() -> Self {
        let lease = crate::messaging::feishu::fixture_lease();
        let root = crate::utils::test_temp::unique_temp_path("feishu-coordinator");
        let program = install_cli(&root);
        let mut worker = Worker::open(root.join("session"), Box::new(NoNative)).unwrap();
        worker.settings_path = Some(root.join("settings.json"));
        settings::save(
            worker.settings_path.as_ref().unwrap(),
            &settings::BusSettings::default(),
        )
        .unwrap();
        worker.attention.client = Client::at(program, Duration::from_secs(10));
        let mut state = worker.state.clone();
        let master = state.ensure_master_room();
        let room = state.create_room("work").unwrap();
        let agent = state
            .create_agent(master, "orch", Provider::ClaudeCode, root.clone(), None)
            .unwrap();
        state.bind_orchestrator(agent, room).unwrap();
        worker.save(state).unwrap();
        let (events, receiver) = mpsc::channel();
        Self {
            worker: Some(worker),
            root,
            events,
            _receiver: receiver,
            lease,
            next_call: 0,
            agent,
            room,
        }
    }

    fn worker(&mut self) -> &mut Worker {
        self.worker.as_mut().unwrap()
    }

    fn quota(&mut self, observed_at: u64, reset: u64) {
        let agent = self.agent;
        self.worker().usage.codex = Some(crate::messaging::coordinator::usage::UsageSnapshot {
            windows: crate::agents::providers::usage::UsageWindows {
                five_hour: Some(crate::agents::providers::usage::UsageWindow {
                    used_percent: 100.0,
                    resets_at: Some(reset),
                    window_minutes: 300,
                }),
                weekly: None,
            },
            read_at_ms: observed_at,
            observed_by_agent: agent,
        });
        let events = self.events.clone();
        self.worker().pump_feishu(&events, observed_at).unwrap();
    }

    fn restart(&mut self) {
        drop(self.worker.take());
        let mut worker = Worker::open(self.root.join("session"), Box::new(NoNative)).unwrap();
        worker.settings_path = Some(self.root.join("settings.json"));
        worker.attention.client = Client::at(self.root.join("lark-cli"), Duration::from_secs(10));
        self.worker = Some(worker);
    }

    fn call(&mut self, method: &str, params: Value) -> Response {
        self.next_call += 1;
        let request = ControlRequest {
            id: format!("attention-{}", self.next_call),
            method: method.into(),
            params,
        };
        let events = self.events.clone();
        self.worker()
            .control_response_with_events(&request, Some(&events))
    }

    fn pump(&mut self) {
        let events = self.events.clone();
        self.worker()
            .pump_feishu(&events, crate::messaging::storage::io::now_ms())
            .unwrap();
    }

    fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            self.pump();
            if !self.worker().attention.status.pending {
                return;
            }
            assert!(Instant::now() < deadline, "validation did not settle");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn mode(&self, value: &str) {
        std::fs::write(self.root.join("mode"), value).unwrap();
    }

    fn calls(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn post(&mut self, text: &str) {
        let agent = self.agent;
        let worker = self.worker();
        let mut state = worker.state.clone();
        let master = state.master_room().unwrap().id;
        state
            .post_to_developer(
                master,
                agent,
                text.into(),
                vec![],
                crate::messaging::storage::io::now_ms(),
            )
            .unwrap();
        worker.save(state).unwrap();
    }

    fn wait_sends(&mut self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            self.pump();
            if self
                .calls()
                .iter()
                .filter(|call| call["args"][0] == "im")
                .count()
                == count
            {
                self.pump();
                return;
            }
            let status = self.worker().attention.status.clone();
            assert!(
                Instant::now() < deadline,
                "expected {count} sends; status={status:?}; calls={:?}",
                self.calls()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.worker.take());
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = self.lease.unlock();
    }
}

#[test]
fn feishu_settings_enable_disable_is_public_async_and_persists_only_after_auth() {
    let mut fixture = Fixture::new();
    let response = fixture.call("settings.feishu_notifications", json!({"on":true}));
    assert!(response.ok, "{response:?}");
    assert!(fixture.worker().attention.status.pending);
    assert!(
        !settings::load(&fixture.root.join("settings.json"))
            .unwrap()
            .feishu_notifications
    );
    fixture.settle();
    assert!(fixture.worker().attention.status.enabled);
    assert!(
        settings::load(&fixture.root.join("settings.json"))
            .unwrap()
            .feishu_notifications
    );
    let off = fixture.call("settings.feishu_notifications", json!({"on":false}));
    assert!(off.ok, "{off:?}");
    assert!(!fixture.worker().attention.status.enabled);
    assert!(
        !settings::load(&fixture.root.join("settings.json"))
            .unwrap()
            .feishu_notifications
    );
    assert!(fixture.calls().iter().all(|call| call["args"][0] == "auth"));
}

#[test]
fn feishu_settings_auth_missing_stays_off_with_visible_reason_and_zero_sends() {
    let mut fixture = Fixture::new();
    fixture.mode("missing");
    let response = fixture.call("settings.feishu_notifications", json!({"on":true}));
    assert!(response.ok, "{response:?}");
    fixture.settle();
    assert!(!fixture.worker().attention.status.enabled);
    assert!(fixture
        .worker()
        .attention
        .status
        .error
        .as_deref()
        .unwrap()
        .contains("authenticated user"));
    assert!(
        !settings::load(&fixture.root.join("settings.json"))
            .unwrap()
            .feishu_notifications
    );
    assert!(fixture.calls().iter().all(|call| call["args"][0] == "auth"));
}

#[test]
fn feishu_settings_refuses_every_destination_parameter_before_invoking_lark() {
    let mut fixture = Fixture::new();
    for field in [
        "recipient",
        "user_id",
        "chat_id",
        "group",
        "feishu_recipient",
    ] {
        let mut params = json!({"on":true});
        params[field] = json!("ou_other");
        let response = fixture.call("settings.feishu_notifications", params);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "invalid_params");
    }
    assert!(fixture.calls().is_empty());
}

#[test]
fn feishu_tagged_master_source_forwards_once_and_untagged_progress_does_not() {
    let mut fixture = Fixture::new();
    fixture.post("[needs_developer_attention] old history must not replay");
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    fixture.post("routine progress");
    fixture.post("[needs_developer_attention] Choose one target");
    fixture.wait_sends(1);
    fixture.post("[needs_developer_attention] Choose one target");
    for _ in 0..10 {
        fixture.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    let calls = fixture.calls();
    let sends: Vec<_> = calls
        .iter()
        .filter(|call| call["args"][0] == "im")
        .collect();
    assert_eq!(sends.len(), 1);
    assert!(sends[0]["body"]
        .as_str()
        .unwrap()
        .contains("Choose one target"));
    assert!(!sends[0]["body"].as_str().unwrap().contains("old history"));
    assert_eq!(sends[0]["args"][5], sends[0]["self"]);
}

#[test]
fn feishu_codex_quota_keeps_original_event_time_instead_of_refresh_time() {
    let mut fixture = Fixture::new();
    let path = fixture.root.join("rollout-quota.jsonl");
    std::fs::write(&path, json!({"timestamp":"2026-10-05T00:00:00Z", "type":"event_msg", "payload":{"type":"token_count", "rate_limits":{"primary":{"used_percent":100.0,"window_minutes":300,"resets_at":4_000_000_000u64}}}}).to_string()).unwrap();
    let agent = fixture.agent;
    fixture.worker().refresh_codex_usage(agent, &path);
    assert_eq!(
        fixture.worker().usage.codex.as_ref().unwrap().read_at_ms,
        1_791_158_400_000
    );
}

#[test]
fn feishu_rate_limit_and_backlog_survive_restart_and_all_sends_are_self_only() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    for index in 0..8 {
        fixture.post(&format!("[needs_developer_attention] Decision {index}"));
    }
    fixture.wait_sends(6);
    fixture.restart();
    fixture.settle();
    for _ in 0..5 {
        fixture.pump();
    }
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call["args"][0] == "im")
            .count(),
        6
    );
    let events = fixture.events.clone();
    let later = crate::messaging::storage::io::now_ms() + 61_000;
    let deadline = Instant::now() + Duration::from_secs(12);
    while fixture
        .calls()
        .iter()
        .filter(|call| call["args"][0] == "im")
        .count()
        < 8
    {
        fixture.worker().pump_feishu(&events, later).unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    for call in fixture
        .calls()
        .iter()
        .filter(|call| call["args"][0] == "im")
    {
        assert_eq!(call["args"][5], call["self"]);
    }
}

#[test]
fn feishu_attempt_save_failure_never_spawns_send() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    fixture.post("[needs_developer_attention] Do not send before durable attempt");
    fixture
        .worker()
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    let events = fixture.events.clone();
    assert!(fixture
        .worker()
        .pump_feishu(&events, crate::messaging::storage::io::now_ms())
        .is_err());
    assert!(fixture.calls().iter().all(|call| call["args"][0] == "auth"));
}

#[test]
fn feishu_failed_send_is_visible_and_never_replayed_after_restart() {
    let mut fixture = Fixture::new();
    fixture.mode("send-error");
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    fixture.post("[needs_developer_attention] Failure outcome");
    fixture.wait_sends(1);
    fixture.restart();
    fixture.settle();
    assert!(fixture.worker().attention.status.error.is_some());
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call["args"][0] == "im")
            .count(),
        1
    );
}

#[test]
fn feishu_only_master_orchestrator_final_and_typed_bus_notices_qualify() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    let agent = fixture.agent;
    let work = fixture.room;
    let worker = fixture.worker();
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    let prompt = crate::messaging::model::Draft {
        text: "question".into(),
        recipient_ids: [agent].into(),
        ..Default::default()
    };
    let request = state
        .submit_message_from(
            master,
            prompt,
            crate::messaging::model::Author::Developer,
            1,
        )
        .unwrap()[0];
    state.finish_for_test(request, "[needs_developer_attention] Final decision", 2);
    state
        .post_to_developer(
            work,
            agent,
            "[needs_developer_attention] Work room does not qualify".into(),
            vec![],
            3,
        )
        .unwrap();
    worker.save(state).unwrap();
    fixture.wait_sends(1);
    let sends = fixture.calls();
    let body = sends.iter().find(|call| call["args"][0] == "im").unwrap()["body"]
        .as_str()
        .unwrap();
    assert!(body.contains("Final decision"));
    assert!(!body.contains("Work room does not qualify"));
}

#[test]
fn feishu_pending_auth_is_responsive_and_off_cancels_before_any_send() {
    let mut fixture = Fixture::new();
    fixture.mode("auth-delay");
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    assert!(fixture.call("settings", json!({})).ok);
    assert!(fixture.worker().attention.status.pending);
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":false}))
            .ok
    );
    fixture.mode("ready");
    fixture.post("[needs_developer_attention] While disabled");
    for _ in 0..20 {
        fixture.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!fixture.worker().attention.status.enabled);
    assert!(fixture.calls().iter().all(|call| call["args"][0] == "auth"));
}

#[test]
fn feishu_observer_auth_failure_never_disables_other_sessions_global_intent() {
    let mut fixture = Fixture::new();
    settings::update(&fixture.root.join("settings.json"), |settings| {
        settings.feishu_notifications = true
    })
    .unwrap();
    fixture.mode("missing");
    fixture.settle();
    assert!(!fixture.worker().attention.status.enabled);
    assert!(fixture.worker().attention.status.error.is_some());
    assert!(
        settings::load(&fixture.root.join("settings.json"))
            .unwrap()
            .feishu_notifications
    );
}

#[test]
fn feishu_explicit_off_then_external_on_ignores_disabled_history() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":false}))
            .ok
    );
    fixture.post("[needs_developer_attention] Disabled history must not replay");
    settings::update(&fixture.root.join("settings.json"), |settings| {
        settings.feishu_notifications = true
    })
    .unwrap();
    fixture.settle();
    fixture.post("[needs_developer_attention] New live message");
    fixture.wait_sends(1);
    let calls = fixture.calls();
    let body = calls.iter().find(|call| call["args"][0] == "im").unwrap()["body"]
        .as_str()
        .unwrap();
    assert!(body.contains("New live message"), "{body}");
}

#[test]
fn feishu_queue_is_bounded_and_overflow_remains_durable_across_restart() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    let now = crate::messaging::storage::io::now_ms();
    let agent = fixture.agent;
    let mut state = fixture.worker().state.clone();
    let master = state.master_room().unwrap().id;
    for index in 0..140 {
        state
            .post_to_developer(
                master,
                agent,
                format!("[needs_developer_attention] Backlog {index}"),
                vec![],
                now,
            )
            .unwrap();
    }
    state.attention_delivery_mut().recent_attempts = (0..6)
        .map(|index| crate::messaging::model::AttentionAttempt {
            source: format!("prior-{index}"),
            signature: format!("prior-{index}"),
            at_ms: now,
        })
        .collect();
    fixture.worker().save(state).unwrap();
    fixture.pump();
    assert_eq!(fixture.worker().attention.queue.len(), QUEUE_LIMIT);
    assert_eq!(fixture.worker().state.attention_sources().count(), 140);
    assert!(fixture
        .worker()
        .state
        .attention_delivery()
        .handled
        .is_empty());
    assert_eq!(
        fixture.worker().settings_json()["feishu"]["queued"],
        140,
        "the durable backlog must be visible"
    );
    fixture.restart();
    fixture.settle();
    assert_eq!(fixture.worker().attention.queue.len(), QUEUE_LIMIT);
    assert_eq!(fixture.worker().state.attention_sources().count(), 140);
    assert!(fixture.calls().iter().all(|call| call["args"][0] == "auth"));
}

#[test]
fn feishu_interrupted_attempt_is_visible_unknown_and_never_replayed() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    fixture.mode("send-delay");
    fixture.post("[needs_developer_attention] Ambiguous delivery");
    fixture.wait_sends(1);
    assert_eq!(
        fixture
            .worker()
            .state
            .attention_delivery()
            .interrupted
            .len(),
        1
    );
    fixture.restart();
    fixture.mode("ready");
    fixture.settle();
    assert!(fixture
        .worker()
        .attention
        .status
        .error
        .as_deref()
        .unwrap()
        .contains("unknown"));
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call["args"][0] == "im")
            .count(),
        1
    );
}

#[test]
fn feishu_quota_from_a_removed_agent_cannot_break_notification_delivery() {
    let mut fixture = Fixture::new();
    let now = crate::messaging::storage::io::now_ms();
    fixture.worker().usage.codex = Some(crate::messaging::coordinator::usage::UsageSnapshot {
        windows: crate::agents::providers::usage::UsageWindows {
            five_hour: Some(crate::agents::providers::usage::UsageWindow {
                used_percent: 100.0,
                resets_at: Some(now / 1000 + 300),
                window_minutes: 300,
            }),
            weekly: None,
        },
        read_at_ms: now,
        observed_by_agent: AgentId(999),
    });
    let events = fixture.events.clone();
    assert!(fixture.worker().pump_feishu(&events, now).is_ok());
}

#[test]
fn feishu_offline_session_skips_old_history_after_a_new_global_enable_generation() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    fixture.post("[needs_developer_attention] Backlog before global off/on");
    drop(fixture.worker.take());
    settings::update(&fixture.root.join("settings.json"), |settings| {
        settings.feishu_notifications = true;
        settings.feishu_notifications_generation += 1;
    })
    .unwrap();
    fixture.restart();
    fixture.settle();
    for _ in 0..10 {
        fixture.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fixture.calls().iter().all(|call| call["args"][0] == "auth"));
}

#[test]
fn feishu_new_exhausted_quota_window_rearms_without_a_sample_between_windows() {
    let mut fixture = Fixture::new();
    let now = crate::messaging::storage::io::now_ms();
    for (observed, reset) in [(now, now / 1000 + 300), (now + 301_000, now / 1000 + 600)] {
        fixture.quota(observed, reset);
        fixture.quota(observed, reset);
    }
    let master = fixture.worker().state.master_room().unwrap().id;
    assert_eq!(fixture.worker().state.notices(master).count(), 2);
}

#[test]
fn feishu_quota_reset_correction_does_not_start_a_new_exhaustion_episode() {
    let mut fixture = Fixture::new();
    let now = crate::messaging::storage::io::now_ms();
    fixture.quota(now, now / 1000 + 300);
    fixture.quota(now, now / 1000 + 600);
    let master = fixture.worker().state.master_room().unwrap().id;
    assert_eq!(
        fixture.worker().state.notices(master).count(),
        1,
        "a corrected future reset does not prove recovery"
    );
}

#[test]
fn feishu_explicit_on_revalidates_when_global_off_arrives_before_local_poll() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    fixture.settle();
    settings::update(&fixture.root.join("settings.json"), |settings| {
        settings.feishu_notifications = false
    })
    .unwrap();
    assert!(
        fixture
            .call("settings.feishu_notifications", json!({"on":true}))
            .ok
    );
    assert!(fixture.worker().attention.status.pending);
    fixture.settle();
    assert!(fixture.worker().attention.status.enabled);
    assert!(
        settings::load(&fixture.root.join("settings.json"))
            .unwrap()
            .feishu_notifications
    );
}

#[test]
fn feishu_distinct_sessions_with_the_same_directory_basename_have_distinct_delivery_keys() {
    let mut delivered = Vec::new();
    for index in 0..2 {
        let mut fixture = Fixture::new();
        assert!(
            fixture
                .call("settings.feishu_notifications", json!({"on":true}))
                .ok
        );
        fixture.settle();
        fixture.post(&format!(
            "[needs_developer_attention] Session decision {index}"
        ));
        fixture.wait_sends(1);
        let calls = fixture.calls();
        let send = calls.iter().find(|call| call["args"][0] == "im").unwrap();
        delivered.push((
            send["args"][9].clone(),
            send["body"].as_str().unwrap().to_owned(),
            fixture.root.join("session"),
        ));
    }
    assert_ne!(
        delivered[0].0, delivered[1].0,
        "independent sessions must not collide in Feishu idempotency"
    );
    for (_, body, session) in delivered {
        assert!(body.contains(&format!("Session: {}", session.display())));
    }
}
