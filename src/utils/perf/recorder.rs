//! The `bus-perf` thread: one JSONL window per interval and stall reports in between.
//!
//! Files live in `<session data dir>/perf/<role>.jsonl`. Every process of a role
//! appends to the same file through the dev log writer, which locks it and
//! rotates it into gzipped, time-stamped archives; a `bus-perf-writer` thread
//! does the appends so the watchdog never waits on that lock. A line is either
//! `{"kind":"window",...}` (process sample plus every metric active in the
//! window) or `{"kind":"stall",...}` (a loop blocked past the threshold, a move
//! to another stage while still blocked, its end, or a finished stack capture).

#[path = "capture.rs"]
mod capture;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::writer::{WriterHandle, DRAIN_DEADLINE, QUEUE_RECORDS};
use super::{
    context, global, now_us, publish_context, ContextSample, Field, Registry, StallEvent,
    StallKind, TimerWindow,
};
use crate::utils::logging::{ArchivePolicy, Retention, RotatingFileState};

/// How often the watchdog looks for blocked loops.
const WATCH_TICK: Duration = Duration::from_millis(100);
const MIB: f64 = 1024.0 * 1024.0;
/// Machine-wide or this process's CPU at or above this is saturated.
const HIGH_CPU_PCT: f64 = 90.0;
/// A run queue twice the core count means every thread waits for a core.
const HIGH_LOAD_PER_CPU: f64 = 2.0;
const LOAD_WARN_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PerfOptions {
    pub(crate) enabled: bool,
    pub(crate) window: Duration,
    pub(crate) stall_after: Duration,
    /// A stall still blocking after this long gets one stack capture; `None` disables it.
    pub(crate) capture_after: Option<Duration>,
    pub(crate) capture_cooldown: Duration,
    pub(crate) max_bytes: u64,
    /// Rotated perf files are kept within this age and total size.
    pub(crate) archive: ArchivePolicy,
    /// Completed stack dumps are retained by age and total bytes per role.
    pub(crate) capture_retention: ArchivePolicy,
}

impl PerfOptions {
    /// `BUS_PERF=off|0|false` disables recording; `BUS_PERF_STALL_MS` and
    /// `BUS_PERF_CAPTURE_MS` (0 = no capture) tune the thresholds.
    pub(crate) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let millis = |key: &str| lookup(key).and_then(|value| value.trim().parse::<u64>().ok());
        let enabled = lookup("BUS_PERF")
            .is_none_or(|value| !matches!(value.trim(), "0" | "off" | "false" | "no"));
        let capture_after = match millis("BUS_PERF_CAPTURE_MS") {
            Some(0) => None,
            Some(ms) => Some(Duration::from_millis(ms)),
            // `sample` needs about a second to attach; a later start misses short stalls.
            None => Some(Duration::from_secs(1)),
        };
        Self {
            enabled,
            window: Duration::from_secs(1),
            stall_after: Duration::from_millis(
                millis("BUS_PERF_STALL_MS")
                    .filter(|ms| *ms > 0)
                    .unwrap_or(250),
            ),
            capture_after,
            capture_cooldown: Duration::from_secs(600),
            max_bytes: 5 * 1024 * 1024,
            // Volume depends on thread count and active metrics; both caps apply.
            archive: ArchivePolicy {
                max_age: Duration::from_secs(72 * 60 * 60),
                max_total_bytes: 64 * 1024 * 1024,
            },
            capture_retention: ArchivePolicy {
                max_age: Duration::from_secs(72 * 3600),
                max_total_bytes: 64 * 1024 * 1024,
            },
        }
    }
}

/// Raw facts about this process, supplied by `platform` at startup composition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProcessUsage {
    pub(crate) cpu_time: Duration,
    pub(crate) rss_bytes: u64,
    pub(crate) threads: u32,
    pub(crate) open_fds: u32,
    pub(crate) load_1m: Option<f64>,
    pub(crate) cpus: u32,
    /// Machine-wide cumulative CPU ticks as (busy, total).
    pub(crate) system_cpu_ticks: Option<(u64, u64)>,
    pub(crate) swap_used_bytes: Option<u64>,
    pub(crate) thread_cpu: Option<Vec<ThreadUsage>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThreadUsage {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) cpu_time: Duration,
}

#[derive(Clone, Copy)]
pub(crate) struct PerfHooks {
    pub(crate) sample: fn() -> Option<ProcessUsage>,
    pub(crate) machine: super::machine::MachineHook,
    pub(crate) local_hm: fn(i64) -> (u8, u8),
    /// Writes the stacks of process `pid` to the path, blocking until done; false
    /// when unsupported or when no stacks were written. Runs off the watchdog.
    pub(crate) capture_stacks: fn(u32, &Path) -> bool,
}

pub(crate) fn perf_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("perf")
}

/// Owns this process's recorder: dropping it, or [`exit`], finishes it.
pub(crate) struct PerfRecorder(());

impl Drop for PerfRecorder {
    fn drop(&mut self) {
        finish();
    }
}

/// The running watchdog: told when to stop, and how long the drain may take.
struct Running {
    stop: mpsc::Sender<Instant>,
    done: mpsc::Receiver<()>,
}

static RUNNING: std::sync::Mutex<Option<Running>> = std::sync::Mutex::new(None);

/// Stops the watchdog and drains the queued records, all within one deadline
/// of [`DRAIN_DEADLINE`]. A watchdog or writer still blocked then (a stuck log
/// or perf file) is left behind rather than joined, so exiting never waits more.
pub(crate) fn finish() {
    let running = RUNNING.lock().ok().and_then(|mut running| running.take());
    let Some(running) = running else {
        return;
    };
    let deadline = Instant::now() + DRAIN_DEADLINE;
    if running.stop.send(deadline).is_ok() {
        let _ = running
            .done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()));
    }
}

/// Exits the process after finishing the recorder; for direct exits that
/// would otherwise skip the owner's drop.
pub(crate) fn exit(code: i32) -> ! {
    finish();
    std::process::exit(code)
}

/// Starts this process's recorder once; `None` when disabled, already
/// started or unavailable.
pub(crate) fn start(
    data_dir: &Path,
    role: &'static str,
    options: &PerfOptions,
    hooks: PerfHooks,
) -> Option<PerfRecorder> {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !options.enabled || STARTED.swap(true, Ordering::AcqRel) {
        return None;
    }
    let recorder = match Recorder::open(data_dir, role, options, hooks) {
        Ok(recorder) => recorder,
        Err(error) => {
            tracing::warn!(event = "perf.disabled", %error, "perf metrics unavailable");
            return None;
        }
    };
    let (stop, stop_rx) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    std::thread::Builder::new()
        .name("bus-perf".into())
        .spawn(move || {
            recorder.run(global(), &stop_rx);
            let _ = done_tx.send(());
        })
        .ok()?;
    if let Ok(mut running) = RUNNING.lock() {
        *running = Some(Running { stop, done });
    }
    Some(PerfRecorder(()))
}

pub(super) struct Recorder {
    role: &'static str,
    pid: u32,
    dir: PathBuf,
    out: WriterHandle,
    options: PerfOptions,
    hooks: PerfHooks,
    last_cpu: Option<(Instant, Duration)>,
    last_thread_cpu: BTreeMap<u64, (Instant, Duration)>,
    last_system_ticks: Option<(u64, u64)>,
    last_load_warn: Option<Instant>,
    last_capture: BTreeMap<&'static str, Instant>,
    capture_sequence: u64,
    captures: (
        mpsc::Sender<capture::FinishedCapture>,
        mpsc::Receiver<capture::FinishedCapture>,
    ),
    pending_captures: BTreeSet<String>,
    wake_lag_max: Option<Duration>,
    machine: super::machine::Sampler,
    capture_levels: BTreeMap<&'static str, (u64, u64)>,
    capture_suppressed: BTreeMap<&'static str, (u64, &'static str)>,
}

impl Recorder {
    pub(super) fn open(
        data_dir: &Path,
        role: &'static str,
        options: &PerfOptions,
        hooks: PerfHooks,
    ) -> std::io::Result<Self> {
        let dir = perf_dir(data_dir);
        std::fs::create_dir_all(&dir)?;
        let out = RotatingFileState::open(
            dir.join(format!("{role}.jsonl")),
            options.max_bytes,
            Retention::Archive(options.archive),
        )?;
        let out = WriterHandle::spawn(role, out, QUEUE_RECORDS)?;
        Ok(Self {
            role,
            pid: std::process::id(),
            dir,
            out,
            options: options.clone(),
            hooks,
            last_cpu: None,
            last_thread_cpu: BTreeMap::new(),
            last_system_ticks: None,
            last_load_warn: None,
            last_capture: BTreeMap::new(),
            capture_sequence: 0,
            captures: mpsc::channel(),
            pending_captures: BTreeSet::new(),
            wake_lag_max: None,
            machine: super::machine::Sampler::spawn(hooks.machine),
            capture_levels: BTreeMap::new(),
            capture_suppressed: BTreeMap::new(),
        })
    }

    /// Watches and samples until `stop` brings the drain deadline, then drains
    /// the writer until it.
    fn run(mut self, registry: &Registry, stop: &mpsc::Receiver<Instant>) {
        self.prune_captures();
        let mut record = self.base("run_start");
        record.insert("build_sha".into(), json!(env!("BUS_BUILD_SHA")));
        record.insert("profile".into(), json!(env!("BUS_BUILD_PROFILE")));
        record.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
        record.insert("dev_tools".into(), json!(cfg!(feature = "dev-tools")));
        record.insert("monotonic_us".into(), json!(now_us()));
        self.append(&Value::Object(record));
        let mut next_window = Instant::now() + self.options.window;
        loop {
            match stop.try_recv() {
                Ok(deadline) => {
                    self.out.shutdown_until(deadline);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let slept_from = Instant::now();
            std::thread::sleep(WATCH_TICK);
            let now = Instant::now();
            self.note_wake(WATCH_TICK, now.duration_since(slept_from));
            self.watch(now, now_us(), registry);
            if now >= next_window {
                let usage = (self.hooks.sample)();
                self.write_window(now, usage, registry);
                next_window = now + self.options.window;
            }
        }
    }

    /// A late wake-up means this process, or the whole machine, was not getting CPU.
    pub(super) fn note_wake(&mut self, expected: Duration, actual: Duration) {
        let lag = actual.saturating_sub(expected);
        self.wake_lag_max = Some(self.wake_lag_max.map_or(lag, |max| max.max(lag)));
    }

    pub(super) fn watch(&mut self, now: Instant, now_us: u64, registry: &Registry) {
        while let Ok(finished) = self.captures.1.try_recv() {
            self.finish_capture(finished);
        }
        for watch in registry.watches() {
            if let Some(event) = watch.check(now_us, self.options.stall_after) {
                log_stall(&event);
                let began_at = time::OffsetDateTime::now_utc()
                    .unix_timestamp()
                    .saturating_sub((event.busy_ms / 1000) as i64);
                let (hour, minute) = (self.hooks.local_hm)(began_at);
                super::recovery::observe(&event, hour, minute);
                let record = self.stall_record(&event);
                self.append(&record);
            }
            self.consider_capture(now, now_us, watch);
        }
    }

    pub(super) fn write_window(
        &mut self,
        now: Instant,
        usage: Option<ProcessUsage>,
        registry: &Registry,
    ) {
        let record = self.window_record(now, usage, registry);
        self.append(&record);
    }

    pub(super) fn window_record(
        &mut self,
        now: Instant,
        usage: Option<ProcessUsage>,
        registry: &Registry,
    ) -> Value {
        let mut record = self.base("window");
        if let Some(message) = super::operation::snapshot() {
            record.insert("active_message".into(), message);
        }
        let queues = super::queue::snapshots();
        if !queues.is_empty() {
            record.insert("event_queues".into(), json!(queues));
        }
        if let Some(machine) = self.machine.take() {
            record.insert("machine".into(), machine);
        }
        if let Some(usage) = usage {
            self.insert_usage(&mut record, now, usage);
        } else {
            self.last_thread_cpu.clear();
        }
        if let Some(lag) = self.wake_lag_max.take() {
            record.insert(
                "watchdog_lag_max_ms".into(),
                json!(round_ms(lag.as_micros())),
            );
        }
        let timers: Map<String, Value> = registry
            .timers()
            .into_iter()
            .chain(
                registry
                    .watches()
                    .into_iter()
                    .map(|watch| watch.iterations()),
            )
            .filter_map(|timer| Some((timer.name.to_string(), timer_json(timer.take()?))))
            .collect();
        let gauges: Map<String, Value> = registry
            .gauges()
            .into_iter()
            .filter_map(|gauge| {
                let window = gauge.take()?;
                Some((
                    gauge.name.to_string(),
                    json!({"max": window.max, "last": window.last}),
                ))
            })
            .collect();
        let counters: Map<String, Value> = registry
            .counters()
            .into_iter()
            .filter_map(|counter| Some((counter.name.to_string(), json!(counter.take()?))))
            .collect();
        for (key, section) in [
            ("timers", timers),
            ("gauges", gauges),
            ("counters", counters),
        ] {
            if !section.is_empty() {
                record.insert(key.into(), Value::Object(section));
            }
        }
        Value::Object(record)
    }

    fn insert_usage(&mut self, record: &mut Map<String, Value>, now: Instant, usage: ProcessUsage) {
        let mut next_thread_cpu = BTreeMap::new();
        if let Some(threads) = usage.thread_cpu {
            let threads: Vec<Value> = threads.into_iter().map(|thread| {
                let mut sample = json!({"id": thread.id, "name": thread.name, "cpu_ms": round_ms(thread.cpu_time.as_micros())});
                if let Some(previous) = self.last_thread_cpu.get(&thread.id).copied().filter(|(_, cpu)| *cpu <= thread.cpu_time) {
                    let pct = cpu_percent(previous, (now, thread.cpu_time));
                    sample["cpu_pct"] = json!((pct * 10.0).round() / 10.0);
                }
                next_thread_cpu.insert(thread.id, (now, thread.cpu_time));
                sample
            }).collect();
            record.insert("thread_cpu".into(), json!(threads));
        }
        self.last_thread_cpu = next_thread_cpu;
        if let Some(previous) = self.last_cpu {
            let pct = cpu_percent(previous, (now, usage.cpu_time));
            record.insert("cpu_pct".into(), json!((pct * 10.0).round() / 10.0));
        }
        self.last_cpu = Some((now, usage.cpu_time));
        let rss_mb = (usage.rss_bytes as f64 / MIB * 10.0).round() / 10.0;
        record.insert("rss_mb".into(), json!(rss_mb));
        record.insert("threads".into(), json!(usage.threads));
        record.insert("fds".into(), json!(usage.open_fds));
        if let Some(load) = usage.load_1m {
            record.insert("load1".into(), json!((load * 100.0).round() / 100.0));
        }
        record.insert("cpus".into(), json!(usage.cpus));
        if let (Some((busy, total)), Some((last_busy, last_total))) =
            (usage.system_cpu_ticks, self.last_system_ticks)
        {
            let total = total.saturating_sub(last_total);
            if total > 0 {
                let pct = busy.saturating_sub(last_busy) as f64 / total as f64 * 100.0;
                record.insert("sys_cpu_pct".into(), json!((pct * 10.0).round() / 10.0));
            }
        }
        self.last_system_ticks = usage.system_cpu_ticks;
        if let Some(swap) = usage.swap_used_bytes {
            record.insert("swap_used_mb".into(), json!((swap as f64 / MIB).round()));
        }
        let number = |key: &str| record.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN);
        let sample = ContextSample {
            cpu_pct: number("cpu_pct"),
            rss_mb: number("rss_mb"),
            load1: number("load1"),
            sys_cpu_pct: number("sys_cpu_pct"),
        };
        publish_context(sample);
        self.warn_if_high_load(now, sample, usage.cpus);
    }

    /// The machine or this process is saturated: a likely backdrop for any stall.
    fn warn_if_high_load(&mut self, now: Instant, sample: ContextSample, cpus: u32) {
        let high = sample.sys_cpu_pct >= HIGH_CPU_PCT
            || sample.cpu_pct >= HIGH_CPU_PCT
            || sample.load1 >= f64::from(cpus) * HIGH_LOAD_PER_CPU;
        let cooled = self
            .last_load_warn
            .is_none_or(|last| now.duration_since(last) >= LOAD_WARN_INTERVAL);
        if !high || !cooled {
            return;
        }
        self.last_load_warn = Some(now);
        tracing::warn!(
            event = "perf.high_load",
            role = self.role,
            cpu_pct = %Field(Some(sample.cpu_pct)),
            rss_mb = %Field(Some(sample.rss_mb)),
            load1 = %Field(Some(sample.load1)),
            cpus,
            sys_cpu_pct = %Field(Some(sample.sys_cpu_pct)),
            "high load"
        );
    }

    fn base(&self, kind: &str) -> Map<String, Value> {
        let mut record = Map::new();
        record.insert("kind".into(), json!(kind));
        record.insert("ts".into(), json!(timestamp()));
        record.insert("ts_ms".into(), json!(crate::utils::time::now_ms()));
        record.insert("role".into(), json!(self.role));
        record.insert("pid".into(), json!(self.pid));
        record
    }

    /// Queues one record for the writer thread; never waits on the file.
    pub(super) fn append(&mut self, record: &Value) {
        let mut line = record.to_string();
        line.push('\n');
        self.out.send(line.into_bytes());
    }

    /// Waits until every record appended so far is in the file.
    #[cfg(test)]
    pub(super) fn flush(&self) {
        self.out.flush();
    }
}

fn log_stall(event: &StallEvent) {
    let context = context();
    let cpu_pct = Field(context.map(|c| c.cpu_pct));
    let rss_mb = Field(context.map(|c| c.rss_mb));
    let load1 = Field(context.map(|c| c.load1));
    let sys_cpu_pct = Field(context.map(|c| c.sys_cpu_pct));
    match event.kind {
        StallKind::Began => tracing::warn!(
            event = "perf.stall",
            loop_name = event.loop_name,
            stage = event.stage,
            busy_ms = event.busy_ms,
            %cpu_pct,
            %rss_mb,
            %load1,
            %sys_cpu_pct,
            "loop blocked"
        ),
        StallKind::Moved => tracing::warn!(
            event = "perf.stall.stage",
            loop_name = event.loop_name,
            stage = event.stage,
            busy_ms = event.busy_ms,
            %cpu_pct,
            %rss_mb,
            %load1,
            %sys_cpu_pct,
            "blocked loop moved to another stage"
        ),
        StallKind::Ended => tracing::warn!(
            event = "perf.stall.end",
            loop_name = event.loop_name,
            stage = event.stage,
            busy_ms = event.busy_ms,
            %cpu_pct,
            %rss_mb,
            %load1,
            %sys_cpu_pct,
            "blocked loop resumed"
        ),
    }
}

fn timer_json(window: TimerWindow) -> Value {
    json!({
        "n": window.count,
        "avg_ms": round_ms(u128::from(window.total_us / window.count.max(1))),
        "max_ms": round_ms(u128::from(window.max_us)),
        "ge16ms": window.over_16ms,
        "ge100ms": window.over_100ms,
        "ge1s": window.over_1s,
    })
}

/// Microseconds to milliseconds with one decimal.
fn round_ms(us: u128) -> f64 {
    (us / 100) as f64 / 10.0
}

pub(super) fn cpu_percent(previous: (Instant, Duration), current: (Instant, Duration)) -> f64 {
    let wall = current
        .0
        .saturating_duration_since(previous.0)
        .as_secs_f64();
    if wall <= 0.0 {
        return 0.0;
    }
    current.1.saturating_sub(previous.1).as_secs_f64() / wall * 100.0
}

fn timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn compact_timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

#[cfg(test)]
#[path = "tests/recorder_test.rs"]
mod recorder_test;
