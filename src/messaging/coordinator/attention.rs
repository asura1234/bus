//! Self-only delivery. History is the durable backlog; only 128 bodies are queued.
mod completion;
use super::{mpsc, BusEvent, Worker};
use crate::messaging::{
    feishu::{Authenticated, Cancellation, Client, Message, SendFailure},
    model::AttentionAttempt,
    prefs::settings,
};
use serde::Serialize;
use std::{
    collections::{BTreeSet, VecDeque},
    thread::JoinHandle,
};

const DEDUPE_MS: u64 = 10 * 60_000;
const RATE_MS: u64 = 60_000;
const RATE_LIMIT: usize = 6;
const QUEUE_LIMIT: usize = 128;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct Status {
    pub(crate) enabled: bool,
    pub(crate) pending: bool,
    pub(crate) queued: usize,
    pub(crate) rate_limited: bool,
    pub(crate) error: Option<String>,
}

enum Completion {
    Auth(Result<Authenticated, String>, bool),
    Send(String, Result<(), SendFailure>),
}

#[derive(Default)]
pub(super) struct Runtime {
    pub(super) status: Status,
    pub(super) client: Client,
    observed: Option<(bool, u64)>,
    rebaseline: bool,
    authenticated: Option<Authenticated>,
    cancel: Cancellation,
    job: Option<JoinHandle<Completion>>,
    retired: Vec<JoinHandle<Completion>>,
    queue: VecDeque<Message>,
    published: Status,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(job) = self.job.take() {
            self.retired.push(job);
        }
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
        self.authenticated = None;
        self.queue.clear();
        self.status = Status::default();
    }

    fn start(&mut self, explicit: bool) {
        self.stop();
        let client = self.client.clone();
        let cancel = self.cancel.clone();
        self.job = Some(std::thread::spawn(move || {
            Completion::Auth(client.authenticate(cancel), explicit)
        }));
        self.status.pending = true;
    }

    fn publish(&mut self, events: &mpsc::Sender<BusEvent>) {
        if self.status != self.published {
            let _ = events.send(BusEvent::FeishuStatusChanged(self.status.clone()));
            self.published = self.status.clone();
        }
    }

    fn pump(
        &mut self,
        worker: &mut Worker,
        events: &mpsc::Sender<BusEvent>,
        now: u64,
    ) -> Result<(), String> {
        let mut state = worker.state.clone();
        for (key, agent, active) in worker.usage.exhausted(now) {
            if state
                .agent(agent)
                .is_none_or(|agent| agent.deletion_pending)
            {
                continue;
            }
            state.attention_episode(key.clone(), active, agent, &format!("Provider usage limit reached ({key}). Wait for reset or choose an available provider."), now).map_err(|error| error.to_string())?;
        }
        if state != worker.state {
            worker.save(state)?;
        }
        let desired = worker
            .settings_path
            .as_deref()
            .and_then(|path| settings::load(path).ok())
            .map(|s| (s.feishu_notifications, s.feishu_notifications_generation))
            .unwrap_or((false, 0));
        if self.observed != Some(desired) {
            self.rebaseline = self.observed.is_some()
                || worker.state.attention_delivery().generation != desired.1;
            self.observed = Some(desired);
            if desired.0 {
                self.start(false);
            } else {
                self.stop();
            }
        }
        let mut index = 0;
        while index < self.retired.len() {
            if self.retired[index].is_finished() {
                let _ = self.retired.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        if self.job.as_ref().is_some_and(|job| job.is_finished()) {
            let job = self.job.take().ok_or("Feishu worker missing")?;
            self.complete(
                job.join().map_err(|_| "Feishu worker failed")?,
                worker,
                events,
            )?;
        }
        if self.status.enabled && worker.storage_pause.is_none() {
            self.deliver(worker, now)?;
        }
        if self.status.enabled {
            let delivery = worker.state.attention_delivery();
            self.status.queued = worker
                .state
                .attention_sources()
                .filter(|(source, _, _)| !delivery.handled.contains(source))
                .count();
            self.status.rate_limited = self.status.queued > 0
                && delivery
                    .recent_attempts
                    .iter()
                    .filter(|attempt| now.saturating_sub(attempt.at_ms) < RATE_MS)
                    .count()
                    >= RATE_LIMIT;
        }
        self.publish(events);
        Ok(())
    }

    fn deliver(&mut self, worker: &mut Worker, now: u64) -> Result<(), String> {
        let machine = crate::platform::hostname().unwrap_or_else(|| "unknown".into());
        let session = worker.data_dir.to_string_lossy();
        let queued: BTreeSet<_> = self
            .queue
            .iter()
            .map(|message| message.source.clone())
            .collect();
        let available = QUEUE_LIMIT.saturating_sub(self.queue.len());
        self.queue.extend(
            worker
                .state
                .attention_sources()
                .filter(|(source, _, _)| {
                    !worker.state.attention_delivery().handled.contains(source)
                        && !queued.contains(source)
                })
                .take(available)
                .map(|(source, origin, text)| {
                    Message::new(&machine, &session, source, &origin, text)
                }),
        );
        if self.job.is_some() {
            return Ok(());
        }
        let mut state = worker.state.clone();
        state
            .attention_delivery_mut()
            .recent_attempts
            .retain(|attempt| now.saturating_sub(attempt.at_ms) < DEDUPE_MS);
        while let Some(message) = self.queue.front() {
            let delivery = state.attention_delivery();
            let duplicate = delivery
                .recent_attempts
                .iter()
                .any(|attempt| attempt.signature == message.signature);
            if !duplicate
                && delivery
                    .recent_attempts
                    .iter()
                    .filter(|attempt| now.saturating_sub(attempt.at_ms) < RATE_MS)
                    .count()
                    >= RATE_LIMIT
            {
                break;
            }
            let Some(message) = self.queue.pop_front() else {
                break;
            };
            let delivery = state.attention_delivery_mut();
            delivery.handled.insert(message.source.clone());
            if duplicate {
                continue;
            }
            delivery.interrupted.insert(message.source.clone());
            delivery.recent_attempts.push(AttentionAttempt {
                source: message.source.clone(),
                signature: message.signature.clone(),
                at_ms: now,
            });
            worker.save(state)?;
            let authenticated = self
                .authenticated
                .as_ref()
                .ok_or("Feishu self identity unavailable")?
                .clone();
            self.job = Some(std::thread::spawn(move || {
                Completion::Send(message.source.clone(), authenticated.send(&message))
            }));
            return Ok(());
        }
        worker.save(state)
    }
}

impl Worker {
    pub(super) fn set_feishu_notifications(
        &mut self,
        on: bool,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let path = self
            .settings_path
            .clone()
            .ok_or("Bus settings location unavailable")?;
        let saved = settings::load(&path)?;
        let desired = (
            saved.feishu_notifications,
            saved.feishu_notifications_generation,
        );
        if on
            && (self.attention.status.enabled || self.attention.status.pending)
            && self.attention.observed == Some(desired)
        {
            return Ok(());
        }
        let mut runtime = std::mem::take(&mut self.attention);
        let result = (|| {
            if on {
                runtime.observed = Some(desired);
                runtime.start(true);
            } else {
                runtime.stop();
                let saved =
                    settings::update(&path, |settings| settings.feishu_notifications = false)?;
                runtime.observed = Some((false, saved.feishu_notifications_generation));
                let _ = events.send(BusEvent::SettingsChanged(saved));
            }
            Ok(())
        })();
        runtime.publish(events);
        self.attention = runtime;
        result
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
