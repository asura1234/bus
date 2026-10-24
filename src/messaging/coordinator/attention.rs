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
