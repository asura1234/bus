//! Waiting-on-you DMs run in one cancellable background worker; UI and coordinator never wait.
use super::{mpsc, BusEvent, Worker};
use crate::messaging::{
    feishu::{waiting::WaitingNotifications, Cancellation, Client},
    model::BusState,
    prefs::settings::{self, BusSettings},
};
use serde::Serialize;
use std::thread::JoinHandle;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct Status {
    pub(crate) enabled: bool,
    pub(crate) pending: bool,
    pub(crate) queued: usize,
    pub(crate) rate_limited: bool,
    pub(crate) error: Option<String>,
}

#[derive(Default)]
pub(super) struct Runtime {
    pub(super) status: Status,
    pub(super) client: Client,
    config: Option<(bool, String, Option<String>)>,
    previous: Option<(u64, BusState)>,
    waiting: WaitingNotifications,
    cancel: Cancellation,
    job: Option<JoinHandle<Result<(), String>>>,
    retired: Vec<JoinHandle<Result<(), String>>>,
    published: Status,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
        for job in self.retired.drain(..) {
            let _ = job.join();
        }
    }
}

impl Runtime {
    fn stop(&mut self) {
        self.cancel.cancel();
        if let Some(job) = self.job.take() {
            self.retired.push(job);
        }
        self.waiting.finish(false);
        self.cancel = Cancellation::default();
        self.waiting.configure(false);
        self.status = Status::default();
    }

    fn publish(&mut self, events: &mpsc::Sender<BusEvent>) {
        if self.status != self.published {
            let _ = events.send(BusEvent::FeishuStatusChanged(self.status.clone()));
            self.published = self.status.clone();
        }
    }

    fn collect_completion(&mut self) {
        if self.job.as_ref().is_some_and(|job| job.is_finished()) {
            if let Some(job) = self.job.take() {
                match job
                    .join()
                    .unwrap_or_else(|_| Err("Feishu notification worker panicked".into()))
                {
                    Ok(()) => {
                        self.waiting.finish(true);
                        self.status.error = None;
                        tracing::info!(event = "bus.feishu.sent", "Feishu notification delivered");
                    }
                    Err(error) => {
                        self.waiting.finish(false);
                        tracing::warn!(event = "bus.feishu.failed", %error, "Feishu notification failed; no automatic retry");
                        self.status.error = Some(error);
                    }
                }
            }
            self.status.pending = false;
        }
    }

    fn pump(
        &mut self,
        worker: &mut Worker,
        events: &mpsc::Sender<BusEvent>,
        now: u64,
    ) -> Result<(), String> {
        // Keep quota attention inside Bus even though only explicit waiting tags send DMs.
        let mut state = worker.state.clone();
        for (key, agent, active) in worker.usage.exhausted(now) {
            if state.agent(agent).is_none_or(|a| a.deletion_pending) {
                continue;
            }
            state.attention_episode(key.clone(), active, agent, &format!("Provider usage limit reached ({key}). Wait for reset or choose an available provider."), now).map_err(|e| e.to_string())?;
        }
        if state != worker.state {
            worker.save(state)?;
        }
        let saved = match worker
            .settings_path
            .as_deref()
            .map(settings::load)
            .transpose()
        {
            Ok(saved) => saved.unwrap_or_else(|| BusSettings {
                feishu_notifications: false,
                ..Default::default()
            }),
            Err(error) => {
                if self.status.error.as_deref() != Some(&error) {
                    tracing::warn!(event = "bus.feishu.settings_failed", %error, "Feishu notifications paused");
                }
                self.stop();
                self.config = None;
                self.previous = Some((worker.revision, worker.state.clone()));
                self.status.error = Some(error);
                self.publish(events);
                return Ok(());
            }
        };
        let desired = (
            worker.settings_path.is_some(),
            saved.feishu_recipient.clone(),
            saved.feishu_cli_path.clone(),
        );
        if self.config.as_ref() != Some(&desired) {
            if self.config.is_none() {
                // Startup history is the baseline. Later settings edits cannot drop new alerts.
                self.previous = Some((worker.revision, worker.state.clone()));
            }
            self.status.enabled = desired.0;
            self.waiting.configure(desired.0);
            self.config = Some(desired);
        }
        self.retired.retain(|job| !job.is_finished());
        self.collect_completion();
        if let Some((revision, previous)) = self.previous.as_ref() {
            if *revision != worker.revision {
                self.waiting.observe(previous, &worker.state, now);
                self.previous = Some((worker.revision, worker.state.clone()));
            }
        }
        if self.job.is_none() && worker.storage_pause.is_none() {
            if let Some(text) = self.waiting.take(now) {
                let client = self.client.clone();
                let cancel = self.cancel.clone();
                let recipient = saved.feishu_recipient;
                let program = saved.feishu_cli_path;
                let key = format!(
                    "bus-{}-{}",
                    std::process::id(),
                    crate::messaging::storage::io::now_ns()
                );
                match std::thread::Builder::new()
                    .name("bus-feishu".into())
                    .spawn(move || {
                        client.send(&recipient, program.as_deref(), &text, &key, &cancel)
                    }) {
                    Ok(job) => {
                        self.job = Some(job);
                        self.status.pending = true;
                    }
                    Err(error) => {
                        self.waiting.finish(false);
                        tracing::warn!(event = "bus.feishu.failed", %error, "Could not start Feishu notification worker");
                        self.status.error = Some(error.to_string());
                    }
                }
            }
        }
        self.publish(events);
        Ok(())
    }
}

impl Worker {
    pub(super) fn set_feishu_notifications(
        &mut self,
        on: bool,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        if !on {
            return Err("Developer notifications are always on and cannot be disabled".into());
        }
        let path = self
            .settings_path
            .as_ref()
            .ok_or("Bus settings location unavailable")?;
        let saved = settings::update(path, |s| s.feishu_notifications = true)?;
        let _ = events.send(BusEvent::SettingsChanged(saved));
        self.pump_feishu(events, crate::messaging::storage::io::now_ms())
    }
    pub(super) fn pump_feishu(
        &mut self,
        events: &mpsc::Sender<BusEvent>,
        now_ms: u64,
    ) -> Result<(), String> {
        let mut runtime = std::mem::take(&mut self.attention);
        let result = runtime.pump(self, events, now_ms);
        self.attention = runtime;
        result
    }
}

#[cfg(all(test, unix))]
#[path = "tests/attention_test.rs"]
mod tests;

#[cfg(all(test, unix))]
mod quota_page {
    use crate::agents::providers::usage::{UsageWindow, UsageWindows};
    use crate::messaging::coordinator::usage::UsageSnapshot;
    use crate::messaging::model::Provider;
    use crate::messaging::native::{Transport, TransportError};

    struct NoNative;
    impl Transport for NoNative {
        fn request(
            &mut self,
            _: crate::messaging::coordinator::Method,
        ) -> Result<crate::messaging::coordinator::ResponseResult, TransportError> {
            panic!("quota pages must not drive a terminal");
        }
    }

    fn exhausted(resets_at: u64) -> UsageWindow {
        UsageWindow {
            used_percent: 100.0,
            resets_at: Some(resets_at),
            window_minutes: 300,
        }
    }

    #[test]
    fn three_samples_of_one_quota_key_page_once_and_a_new_reset_pages_again() {
        let root = crate::utils::test_temp::unique_temp_path("quota-page");
        let mut worker = super::Worker::open(root.join("session"), Box::new(NoNative)).unwrap();
        let mut state = worker.state.clone();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let agent = state
            .create_agent(master, "orch", Provider::ClaudeCode, root.clone(), None)
            .unwrap();
        state.bind_orchestrator(agent, work).unwrap();
        worker.save(state).unwrap();
        let start = 1_793_364_695_031;
        worker.usage.claude = Some(UsageSnapshot {
            windows: UsageWindows {
                five_hour: Some(exhausted(1_793_368_200)),
                weekly: None,
            },
            read_at_ms: start,
            observed_by_agent: agent,
        });
        let (events, _receiver) = std::sync::mpsc::channel();
        for minute in 0..3 {
            worker
                .pump_feishu(&events, start + minute * 60_000)
                .unwrap();
        }
        let notices = |worker: &super::Worker| {
            let master = worker.state.master_room().unwrap().id;
            worker
                .state
                .notices(master)
                .map(|notice| notice.text.clone())
                .collect::<Vec<_>>()
        };
        let first = notices(&worker);
        assert_eq!(
            first.len(),
            1,
            "three samples of quota:claude:five_hour:1793368200 one minute apart: {first:?}"
        );
        assert!(
            first[0].contains("quota:claude:five_hour:1793368200"),
            "{first:?}"
        );
        worker.usage.claude.as_mut().unwrap().windows.five_hour = Some(exhausted(1_793_400_000));
        worker.usage.claude.as_mut().unwrap().read_at_ms = start + 3 * 60_000;
        worker.pump_feishu(&events, start + 3 * 60_000).unwrap();
        let next = notices(&worker);
        assert_eq!(
            next.len(),
            2,
            "a new reset timestamp must page once more: {next:?}"
        );
        assert!(
            next[1].contains("quota:claude:five_hour:1793400000"),
            "{next:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn returning_to_an_active_reset_does_not_page_twice() {
        let root = crate::utils::test_temp::unique_temp_path("quota-return");
        let mut worker = super::Worker::open(root.join("session"), Box::new(NoNative)).unwrap();
        let mut state = worker.state.clone();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let agent = state
            .create_agent(master, "orch", Provider::ClaudeCode, root.clone(), None)
            .unwrap();
        state.bind_orchestrator(agent, work).unwrap();
        worker.save(state).unwrap();
        let start = 1_793_364_695_031;
        let (events, _receiver) = std::sync::mpsc::channel();
        let samples: [(Option<u64>, Option<u64>, u64); 4] = [
            (Some(1_793_368_200), None, 0),
            (Some(1_793_368_200), Some(1_793_368_200), 30_000),
            (Some(1_793_400_000), Some(1_793_368_200), 60_000),
            (Some(1_793_368_200), Some(1_793_368_200), 90_000),
        ];
        for (five_hour, weekly, offset) in samples {
            let at = start + offset;
            worker.usage.claude = Some(UsageSnapshot {
                windows: UsageWindows {
                    five_hour: five_hour.map(exhausted),
                    weekly: weekly.map(exhausted),
                },
                read_at_ms: at,
                observed_by_agent: agent,
            });
            worker.pump_feishu(&events, at).unwrap();
        }
        let notices = {
            let master = worker.state.master_room().unwrap().id;
            worker
                .state
                .notices(master)
                .map(|notice| notice.text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(notices.len(), 3, "{notices:?}");
        assert!(
            notices[0].contains("quota:claude:five_hour:1793368200"),
            "{notices:?}"
        );
        assert!(
            notices[1].contains("quota:claude:weekly:1793368200"),
            "{notices:?}"
        );
        assert!(
            notices[2].contains("quota:claude:five_hour:1793400000"),
            "{notices:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
