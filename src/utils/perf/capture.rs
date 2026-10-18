//! Stack captures and retention stay on background threads.
use super::{compact_timestamp, Recorder};
use crate::utils::perf::{global, now_us};
use crate::utils::perf::{LoopWatch, StallEvent, StallKind};
use serde_json::{json, Map, Value};
use std::sync::atomic::Ordering;
use std::time::Instant;
use std::time::SystemTime;

#[derive(Clone)]
pub(super) struct FinishedCapture {
    loop_name: &'static str,
    stage: &'static str,
    busy_ms: u64,
    stall_id: u64,
    threshold_ms: u64,
    file: String,
    ok: bool,
    requested_at: SystemTime,
    requested_us: u64,
    started_at: SystemTime,
    started_us: u64,
    ended_at: SystemTime,
    ended_us: u64,
}
impl Recorder {
    pub(super) fn consider_capture(&mut self, now: Instant, mono: u64, watch: &LoopWatch) {
        let Some(busy_ms) = watch.ongoing_stall_ms(mono) else {
            return;
        };
        let Some(after) = self.options.capture_after else {
            return;
        };
        let initial = after.as_millis().min(u128::from(u64::MAX)) as u64;
        let level = [initial, 5000, 15000, 30000]
            .into_iter()
            .filter(|t| *t >= initial && busy_ms >= *t)
            .max()
            .unwrap_or(0);
        if level == 0 {
            return;
        }
        let id = watch.reported.load(Ordering::Acquire);
        let previous = self.capture_levels.get(watch.name).copied();
        if previous.is_some_and(|(stall, captured)| stall == id && captured >= level) {
            return;
        }
        let cooled = self.last_capture.get(watch.name).is_none_or(|last| {
            now.saturating_duration_since(*last) >= self.options.capture_cooldown
        });
        let reason = if !self.pending_captures.is_empty() {
            Some("capture_in_flight")
        } else if !cooled && level == initial {
            Some("cooldown")
        } else {
            None
        };
        if let Some(reason) = reason {
            if self.capture_suppressed.insert(watch.name, (id, reason)) != Some((id, reason)) {
                let mut record = self.base("stall");
                record.extend([
                    ("state".into(), json!("capture_suppressed")),
                    ("loop".into(), json!(watch.name)),
                    ("stall_id".into(), json!(id)),
                    ("reason".into(), json!(reason)),
                    ("threshold_ms".into(), json!(level)),
                    (
                        "last_capture_age_ms".into(),
                        json!(self
                            .last_capture
                            .get(watch.name)
                            .map(|last| now.saturating_duration_since(*last).as_millis())),
                    ),
                ]);
                self.append(&Value::Object(record));
            }
            return;
        }
        self.capture_levels.insert(watch.name, (id, level));
        self.capture_for(watch.name, watch.stage(), busy_ms, id, level);
        self.last_capture.insert(watch.name, now);
    }
    #[cfg(test)]
    pub(super) fn capture(&mut self, name: &'static str, stage: &'static str, busy_ms: u64) {
        self.capture_for(name, stage, busy_ms, 0, busy_ms);
    }
    fn capture_for(
        &mut self,
        loop_name: &'static str,
        stage: &'static str,
        busy_ms: u64,
        stall_id: u64,
        threshold_ms: u64,
    ) {
        // One sample per PID at a time, regardless of which loop stalled.
        if !self.pending_captures.is_empty() {
            return;
        }
        self.capture_sequence += 1;
        let file = format!(
            "stall-{}-{}-{:016}-{}.txt",
            compact_timestamp(),
            self.pid,
            self.capture_sequence,
            self.role
        );
        let now = SystemTime::now();
        let mono = now_us();
        let mut finished = FinishedCapture {
            loop_name,
            stage,
            busy_ms,
            stall_id,
            threshold_ms,
            file: file.clone(),
            ok: false,
            requested_at: now,
            requested_us: mono,
            started_at: now,
            started_us: mono,
            ended_at: now,
            ended_us: mono,
        };
        let mut record = self.capture_record(&finished, "capture_requested");
        record.insert(
            "last_capture_age_ms".into(),
            json!(self
                .last_capture
                .get(loop_name)
                .map(|last| last.elapsed().as_millis())),
        );
        self.append(&Value::Object(record));
        self.pending_captures.insert(file.clone());
        self.prune_captures();
        let (pid, path, hook) = (
            self.pid,
            self.dir.join(format!("{file}.part")),
            self.hooks.capture_stacks,
        );
        let done = self.captures.0.clone();
        let failure = finished.clone();
        let spawned = std::thread::Builder::new()
            .name("bus-perf-capture".into())
            .spawn(move || {
                finished.started_at = SystemTime::now();
                finished.started_us = now_us();
                finished.ok = capture_with_lease(pid, &path, &path.with_file_name(file), hook);
                finished.ended_at = SystemTime::now();
                finished.ended_us = now_us();
                let _ = done.send(finished);
            });
        if spawned.is_err() {
            self.finish_capture(failure);
        }
    }
    #[cfg(test)]
    pub(super) fn wait_for_captures(&mut self) {
        while !self.pending_captures.is_empty() {
            match self.captures.1.recv() {
                Ok(finished) => self.finish_capture(finished),
                Err(_) => return,
            }
        }
    }
    pub(super) fn finish_capture(&mut self, finished: FinishedCapture) {
        self.pending_captures.remove(&finished.file);
        let mut record = self.capture_record(&finished, "capture");
        let stage = global()
            .watches()
            .into_iter()
            .find(|w| w.name == finished.loop_name)
            .map(|w| w.stage());
        record.insert("stage_at_finish".into(), json!(stage));
        self.append(&Value::Object(record));
        self.prune_captures();
    }
    fn capture_record(&self, c: &FinishedCapture, state: &str) -> Map<String, Value> {
        let mut record = self.base("stall");
        record.extend([
            ("state".into(), json!(state)),
            ("loop".into(), json!(c.loop_name)),
            ("stage".into(), json!(c.stage)),
            ("busy_ms".into(), json!(c.busy_ms)),
            ("stall_id".into(), json!(c.stall_id)),
            ("threshold_ms".into(), json!(c.threshold_ms)),
            ("file".into(), json!(c.file)),
            ("ok".into(), json!(c.ok)),
            ("requested_at".into(), json!(wall(c.requested_at))),
            ("requested_us".into(), json!(c.requested_us)),
        ]);
        if state == "capture" {
            record.extend([
                ("started_at".into(), json!(wall(c.started_at))),
                ("started_us".into(), json!(c.started_us)),
                ("ended_at".into(), json!(wall(c.ended_at))),
                ("ended_us".into(), json!(c.ended_us)),
            ]);
        }
        record
    }
    pub(super) fn prune_captures(&mut self) {
        for eviction in super::super::retention::prune(
            &self.dir,
            self.role,
            &self.pending_captures,
            self.options.capture_retention,
            SystemTime::now(),
        ) {
            let mut record = self.base("capture_evicted");
            record.extend([
                (
                    "file".into(),
                    json!(eviction.file.file_name().and_then(|n| n.to_str())),
                ),
                ("bytes".into(), json!(eviction.bytes)),
                ("reason".into(), json!(eviction.reason)),
            ]);
            self.append(&Value::Object(record));
        }
    }
    pub(super) fn stall_record(&self, event: &StallEvent) -> Value {
        let mut record = self.base("stall");
        let state = match event.kind {
            StallKind::Began => "began",
            StallKind::Moved => "moved",
            StallKind::Ended => "ended",
        };
        record.extend([
            ("state".into(), json!(state)),
            ("loop".into(), json!(event.loop_name)),
            ("stage".into(), json!(event.stage)),
            ("busy_ms".into(), json!(event.busy_ms)),
            ("stall_id".into(), json!(event.stall_id)),
            ("monotonic_us".into(), json!(now_us())),
        ]);
        if let Some(message) = super::super::operation::snapshot() {
            record.insert("active_message".into(), message);
        }
        Value::Object(record)
    }
}
fn wall(at: SystemTime) -> String {
    time::OffsetDateTime::from(at)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// A sidecar lease lets other recorders distinguish a live partial dump from
/// one left by a terminated process, without consulting process command lines.
fn capture_with_lease(
    pid: u32,
    partial: &std::path::Path,
    final_path: &std::path::Path,
    hook: fn(u32, &std::path::Path) -> bool,
) -> bool {
    let lock_path = partial.with_extension("lock");
    let Ok(lease) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&lock_path)
    else {
        return false;
    };
    if lease.try_lock().is_err() {
        return false;
    }
    let captured = hook(pid, partial);
    let renamed = if partial.exists() {
        std::fs::rename(partial, final_path).is_ok()
    } else {
        false
    };
    drop(lease);
    let _ = std::fs::remove_file(lock_path);
    captured && renamed
}
