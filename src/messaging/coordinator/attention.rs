//! Optional bridge sends run serially, after durable per-request outbox admission.
use super::{mpsc, BusEvent, Worker};
use crate::messaging::{
    feishu::{Cancellation, Client, SentMessage},
    model::BusState,
    prefs::settings,
};
use serde::Serialize;
use std::thread::JoinHandle;

type Completion = (String, Result<SentMessage, String>);

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
    previous: Option<(u64, BusState)>,
    cancel: Cancellation,
    job: Option<JoinHandle<Completion>>,
    completion: Option<Completion>,
    retired: Vec<JoinHandle<Completion>>,
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
        self.cancel = Cancellation::default();
        self.status = Status::default();
    }

    fn publish(&mut self, events: &mpsc::Sender<BusEvent>) {
        if self.status != self.published {
            let _ = events.send(BusEvent::FeishuStatusChanged(self.status.clone()));
            self.published = self.status.clone();
        }
    }

    fn collect_completion(&mut self, worker: &mut Worker, finishing: bool) -> Result<(), String> {
        if self.completion.is_none() {
            let job = if self.job.as_ref().is_some_and(|job| job.is_finished()) {
                self.job.take()
            } else {
                self.retired
                    .iter()
                    .position(|job| finishing || job.is_finished())
                    .map(|index| self.retired.swap_remove(index))
            };
            if let Some(job) = job {
                self.completion = job.join().ok();
                self.status.pending = self.job.is_some();
            }
        }
        let Some((code, result)) = self.completion.take() else {
            return Ok(());
        };
        match result {
            Ok(sent) => {
                let mut state = worker.state.clone();
                state.complete_feishu_send(&code, &sent.message_id, &sent.chat_id)?;
                if let Err(error) = worker.save(state) {
                    self.completion = Some((code, Ok(sent)));
                    return Err(error);
                }
                self.status.error = None;
                tracing::info!(
                    event = "bus.feishu.sent",
                    code,
                    "Feishu notification mapping committed"
                );
            }
            Err(error) => {
                let _log_only = crate::utils::error_pipe::suppress(true);
                tracing::warn!(event = "bus.feishu.failed", code, %error, "Feishu send failed; outcome may be uncertain; no automatic retry");
                self.status.error = Some(error);
            }
        }
        Ok(())
    }

    fn pump(
        &mut self,
        worker: &mut Worker,
        events: &mpsc::Sender<BusEvent>,
        now: u64,
    ) -> Result<(), String> {
        // Provider quota attention remains local even without the optional bridge.
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
        // Completed transport results belong to the outbox even when settings
        // become unreadable or the sending worker was cancelled/retired.
        self.collect_completion(worker, false)?;
        let saved = match worker
            .settings_path
            .as_deref()
            .map(settings::load)
            .transpose()
        {
            Ok(saved) => saved,
            Err(error) => {
                let _log_only = crate::utils::error_pipe::suppress(true);
                tracing::warn!(event = "bus.feishu.settings_failed", %error, "Feishu integration paused");
                self.stop();
                self.previous = Some((worker.revision, worker.state.clone()));
                self.status.error = Some(error);
                self.publish(events);
                return Ok(());
            }
        };
        let program = saved
            .as_ref()
            .and_then(|s| self.client.program(s.feishu_bridge_path.as_deref()));
        self.status.enabled = program.is_some();
        if let Some((revision, previous)) = self.previous.as_ref() {
            if *revision != worker.revision && self.status.enabled {
                if let Some(settings) = &saved {
                    let mut next = worker.state.clone();
                    next.prepare_feishu_alerts(
                        previous,
                        &worker.data_dir.to_string_lossy(),
                        &settings.feishu_recipient,
                        now,
                    );
                    worker.save(next)?;
                }
            }
        }
        self.previous = Some((worker.revision, worker.state.clone()));
        if let Some(program) = program {
            if self.job.is_none()
                && self.retired.is_empty()
                && self.completion.is_none()
                && worker.storage_pause.is_none()
            {
                self.start_next(worker, &program)?;
            }
        }
        self.status.queued = worker
            .state
            .feishu
            .outbox
            .values()
            .filter(|record| !record.attempted)
            .count();
        self.publish(events);
        Ok(())
    }

    fn start_next(&mut self, worker: &mut Worker, program: &std::path::Path) -> Result<(), String> {
        let next = worker
            .state
            .feishu
            .outbox
            .values()
            .filter(|record| !record.attempted)
            .min_by_key(|record| (record.created_at_ms, record.source.prompt_id))
            .cloned();
        let Some(record) = next else { return Ok(()) };
        // Persist uncertainty before starting a process. Restart never repeats an attempted send.
        let mut state = worker.state.clone();
        if let Some(saved) = state.feishu.outbox.get_mut(&record.code) {
            saved.attempted = true;
        }
        worker.save(state)?;
        let client = self.client.clone();
        let cancel = self.cancel.clone();
        let program = program.to_string_lossy().into_owned();
        let code = record.code.clone();
        match std::thread::Builder::new()
            .name("bus-feishu".into())
            .spawn(move || {
                let result = client.send(
                    &record.recipient,
                    Some(&program),
                    &record.text,
                    &record.key,
                    &cancel,
                );
                (record.code, result)
            }) {
            Ok(job) => {
                self.job = Some(job);
                self.status.pending = true;
            }
            Err(error) => {
                let _log_only = crate::utils::error_pipe::suppress(true);
                tracing::warn!(event = "bus.feishu.failed", code, %error, "Could not start bridge worker");
                self.status.error = Some(error.to_string());
            }
        }
        Ok(())
    }
}

impl Worker {
    pub(super) fn finish_feishu(&mut self) {
        let mut runtime = std::mem::take(&mut self.attention);
        runtime.stop();
        let result = (|| {
            while !runtime.retired.is_empty() || runtime.completion.is_some() {
                runtime.collect_completion(self, true)?;
            }
            Ok::<(), String>(())
        })();
        self.attention = runtime;
        if let Err(error) = result {
            let _log_only = crate::utils::error_pipe::suppress(true);
            tracing::warn!(event = "bus.feishu.shutdown_commit_failed", %error, "Feishu completion could not be committed before shutdown");
        }
    }

    pub(super) fn set_feishu_notifications(
        &mut self,
        on: bool,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        if !on {
            return Err(
                "Feishu follows bridge availability; this command cannot disable it".into(),
            );
        }
        let path = self
            .settings_path
            .as_ref()
            .ok_or("Bus settings location unavailable")?;
        let saved = settings::update(path, |settings| settings.feishu_notifications = true)?;
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
