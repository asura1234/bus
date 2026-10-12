//! Always-on performance metrics that leave evidence after a UI or server freeze.
//!
//! Hot paths only touch atomics: a [`Timer`], [`Gauge`] or [`Counter`] is a
//! `static` owned by the code it measures, and a [`LoopWatch`] marks when a loop
//! iteration is busy. The `bus-perf` thread in [`recorder`] drains them into a
//! JSONL file once per window and reports loops that stay busy too long.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

mod recorder;
mod writer;

pub(crate) use recorder::{start, PerfHooks, PerfOptions, ProcessUsage};

/// An operation this slow blocks a 60 Hz UI for several frames.
pub(crate) const SLOW_OP: Duration = Duration::from_millis(100);
/// Slow-operation WARNs per timer are limited to one per interval.
const WARN_INTERVAL: Duration = Duration::from_secs(1);

/// Durations of one kind of operation; drained once per window.
pub(crate) struct Timer {
    name: &'static str,
    registered: AtomicBool,
    count: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
    over_16ms: AtomicU64,
    over_100ms: AtomicU64,
    over_1s: AtomicU64,
    last_warn_us: AtomicU64,
    suppressed: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimerWindow {
    pub(crate) count: u64,
    pub(crate) total_us: u64,
    pub(crate) max_us: u64,
    pub(crate) over_16ms: u64,
    pub(crate) over_100ms: u64,
    pub(crate) over_1s: u64,
}

impl Timer {
    pub(crate) const fn new(name: &'static str) -> Self {
        Self {
            name,
            registered: AtomicBool::new(false),
            count: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            max_us: AtomicU64::new(0),
            over_16ms: AtomicU64::new(0),
            over_100ms: AtomicU64::new(0),
            over_1s: AtomicU64::new(0),
            last_warn_us: AtomicU64::new(0),
            suppressed: AtomicU64::new(0),
        }
    }

    /// Records one duration; one at [`SLOW_OP`] or longer also logs a WARN.
    pub(crate) fn record(&'static self, elapsed: Duration) {
        self.record_op(elapsed, "");
    }

    fn register(&'static self) {
        if !self.registered.load(Ordering::Relaxed) && !self.registered.swap(true, Ordering::AcqRel)
        {
            global().add_timer(self);
        }
    }

    fn record_unregistered(&self, elapsed: Duration) {
        let us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_us.fetch_add(us, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
        if us >= 16_000 {
            self.over_16ms.fetch_add(1, Ordering::Relaxed);
        }
        if us >= 100_000 {
            self.over_100ms.fetch_add(1, Ordering::Relaxed);
        }
        if us >= 1_000_000 {
            self.over_1s.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records one operation and logs a WARN naming it when it took
    /// [`SLOW_OP`] or longer, so a window's slow count has a culprit.
    pub(crate) fn record_op(&'static self, elapsed: Duration, detail: &str) {
        self.register();
        self.record_unregistered(elapsed);
        if elapsed >= SLOW_OP {
            self.warn_slow(elapsed, detail);
        }
    }

    /// At most one WARN per timer per [`WARN_INTERVAL`]; the next one counts the rest.
    fn warn_slow(&self, elapsed: Duration, detail: &str) {
        let now = now_us();
        let last = self.last_warn_us.load(Ordering::Relaxed);
        let interval = u64::try_from(WARN_INTERVAL.as_micros()).unwrap_or(u64::MAX);
        if last != 0 && now.saturating_sub(last) < interval
            || self
                .last_warn_us
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
        {
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let context = context();
        tracing::warn!(
            event = "perf.slow_op",
            metric = self.name,
            detail,
            elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            suppressed = self.suppressed.swap(0, Ordering::Relaxed),
            cpu_pct = %Field(context.map(|c| c.cpu_pct)),
            rss_mb = %Field(context.map(|c| c.rss_mb)),
            load1 = %Field(context.map(|c| c.load1)),
            sys_cpu_pct = %Field(context.map(|c| c.sys_cpu_pct)),
            "slow operation"
        );
    }

    /// Records the time until the guard drops.
    pub(crate) fn start(&'static self) -> TimerGuard {
        TimerGuard {
            timer: self,
            started: Instant::now(),
        }
    }

    pub(crate) fn take(&self) -> Option<TimerWindow> {
        let count = self.count.swap(0, Ordering::Relaxed);
        let window = TimerWindow {
            count,
            total_us: self.total_us.swap(0, Ordering::Relaxed),
            max_us: self.max_us.swap(0, Ordering::Relaxed),
            over_16ms: self.over_16ms.swap(0, Ordering::Relaxed),
            over_100ms: self.over_100ms.swap(0, Ordering::Relaxed),
            over_1s: self.over_1s.swap(0, Ordering::Relaxed),
        };
        (count > 0).then_some(window)
    }
}

pub(crate) struct TimerGuard {
    timer: &'static Timer,
    started: Instant,
}

impl Drop for TimerGuard {
    fn drop(&mut self) {
        self.timer.record(self.started.elapsed());
    }
}

/// A sampled level such as a queue depth; the window keeps its max and last value.
pub(crate) struct Gauge {
    name: &'static str,
    registered: AtomicBool,
    observed: AtomicBool,
    max: AtomicU64,
    last: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GaugeWindow {
    pub(crate) max: u64,
    pub(crate) last: u64,
}

impl Gauge {
    pub(crate) const fn new(name: &'static str) -> Self {
        Self {
            name,
            registered: AtomicBool::new(false),
            observed: AtomicBool::new(false),
            max: AtomicU64::new(0),
            last: AtomicU64::new(0),
        }
    }

    pub(crate) fn observe(&'static self, value: u64) {
        if !self.registered.load(Ordering::Relaxed) && !self.registered.swap(true, Ordering::AcqRel)
        {
            global().add_gauge(self);
        }
        self.max.fetch_max(value, Ordering::Relaxed);
        self.last.store(value, Ordering::Relaxed);
        self.observed.store(true, Ordering::Release);
    }

    pub(crate) fn take(&self) -> Option<GaugeWindow> {
        if !self.observed.swap(false, Ordering::AcqRel) {
            return None;
        }
        let last = self.last.load(Ordering::Relaxed);
        let max = self.max.swap(0, Ordering::Relaxed).max(last);
        Some(GaugeWindow { max, last })
    }
}

/// A running total such as bytes read; the window holds the sum since the last one.
pub(crate) struct Counter {
    name: &'static str,
    registered: AtomicBool,
    total: AtomicU64,
}

impl Counter {
    pub(crate) const fn new(name: &'static str) -> Self {
        Self {
            name,
            registered: AtomicBool::new(false),
            total: AtomicU64::new(0),
        }
    }

    pub(crate) fn add(&'static self, value: u64) {
        if value == 0 {
            return;
        }
        if !self.registered.load(Ordering::Relaxed) && !self.registered.swap(true, Ordering::AcqRel)
        {
            global().add_counter(self);
        }
        self.total.fetch_add(value, Ordering::Relaxed);
    }

    pub(crate) fn take(&self) -> Option<u64> {
        let total = self.total.swap(0, Ordering::Relaxed);
        (total > 0).then_some(total)
    }
}

/// Marks a loop's busy iterations so the watchdog can report one that blocks.
///
/// The loop thread only stores atomics and a stage label; the watchdog thread
/// decides when a busy iteration is a stall and when that stall ended.
pub(crate) struct LoopWatch {
    name: &'static str,
    registered: AtomicBool,
    iterations: Timer,
    stage: Mutex<&'static str>,
    /// Start of the current busy iteration in [`now_us`] units; 0 when idle.
    busy_since: AtomicU64,
    /// Start of the iteration the watchdog reported as stalled; 0 when none.
    reported: AtomicU64,
    /// Busy time of the reported iteration, stored by the loop when it ends.
    reported_busy_us: AtomicU64,
    reported_stage: Mutex<&'static str>,
    /// The stage the watchdog last reported for the ongoing stall.
    latest_stage: Mutex<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StallKind {
    Began,
    /// A reported stall is still blocking, now in another stage.
    Moved,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StallEvent {
    pub(crate) kind: StallKind,
    pub(crate) loop_name: &'static str,
    pub(crate) stage: &'static str,
    pub(crate) busy_ms: u64,
}

impl LoopWatch {
    pub(crate) const fn new(name: &'static str) -> Self {
        Self {
            name,
            registered: AtomicBool::new(false),
            iterations: Timer::new(name),
            stage: Mutex::new(""),
            busy_since: AtomicU64::new(0),
            reported: AtomicU64::new(0),
            reported_busy_us: AtomicU64::new(0),
            reported_stage: Mutex::new(""),
            latest_stage: Mutex::new(""),
        }
    }

    /// Starts a busy iteration labelled `stage`; it ends when the guard drops.
    pub(crate) fn enter(&'static self, stage: &'static str) -> LoopGuard {
        if !self.registered.load(Ordering::Relaxed) && !self.registered.swap(true, Ordering::AcqRel)
        {
            global().add_watch(self);
        }
        self.set_stage(stage);
        let started = now_us();
        self.busy_since.store(started, Ordering::Release);
        LoopGuard {
            watch: self,
            started,
            started_at: Instant::now(),
        }
    }

    /// Relabels the current iteration from code that does not hold its guard.
    pub(crate) fn relabel(&self, stage: &'static str) {
        self.set_stage(stage);
    }

    fn set_stage(&self, stage: &'static str) {
        if let Ok(mut current) = self.stage.lock() {
            *current = stage;
        }
    }

    fn stage(&self) -> &'static str {
        self.stage.lock().map_or("", |stage| *stage)
    }

    pub(crate) fn iterations(&self) -> &Timer {
        &self.iterations
    }

    /// Watchdog side: reports a newly blocked iteration, a reported one that
    /// moved on to another stage while still blocking, or its end.
    pub(crate) fn check(&self, now: u64, threshold: Duration) -> Option<StallEvent> {
        let reported = self.reported.load(Ordering::Acquire);
        let busy_since = self.busy_since.load(Ordering::Acquire);
        if reported != 0 && busy_since != reported {
            self.reported.store(0, Ordering::Release);
            // The loop can end the iteration between the watchdog's two stores; the
            // time since it began is then the closest bound the watchdog has.
            let busy_us = match self.reported_busy_us.swap(0, Ordering::AcqRel) {
                0 => now.saturating_sub(reported),
                busy_us => busy_us,
            };
            let stage = self.reported_stage.lock().map_or("", |stage| *stage);
            return Some(self.event(StallKind::Ended, stage, busy_us));
        }
        if busy_since == 0 {
            return None;
        }
        if reported == busy_since {
            let stage = self.stage();
            let mut latest = self.latest_stage.lock().ok()?;
            if *latest == stage {
                return None;
            }
            *latest = stage;
            return Some(self.event(StallKind::Moved, stage, now.saturating_sub(reported)));
        }
        let busy_us = now.saturating_sub(busy_since);
        if busy_us < u64::try_from(threshold.as_micros()).unwrap_or(u64::MAX) {
            return None;
        }
        let stage = self.stage();
        for slot in [&self.reported_stage, &self.latest_stage] {
            if let Ok(mut reported_stage) = slot.lock() {
                *reported_stage = stage;
            }
        }
        self.reported.store(busy_since, Ordering::Release);
        Some(self.event(StallKind::Began, stage, busy_us))
    }

    /// How long the reported stall has lasted, while it is still blocking.
    pub(crate) fn ongoing_stall_ms(&self, now: u64) -> Option<u64> {
        let reported = self.reported.load(Ordering::Acquire);
        (reported != 0 && self.busy_since.load(Ordering::Acquire) == reported)
            .then(|| now.saturating_sub(reported) / 1_000)
    }

    fn event(&self, kind: StallKind, stage: &'static str, busy_us: u64) -> StallEvent {
        StallEvent {
            kind,
            loop_name: self.name,
            stage,
            busy_ms: busy_us / 1_000,
        }
    }
}

pub(crate) struct LoopGuard {
    watch: &'static LoopWatch,
    started: u64,
    started_at: Instant,
}

impl LoopGuard {
    /// Relabels the iteration so a stall names the step that blocked.
    pub(crate) fn stage(&self, stage: &'static str) {
        self.watch.set_stage(stage);
    }
}

impl Drop for LoopGuard {
    fn drop(&mut self) {
        let elapsed = self.started_at.elapsed();
        let watch = self.watch;
        if watch.reported.load(Ordering::Acquire) == self.started {
            let us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
            watch.reported_busy_us.store(us, Ordering::Release);
        }
        watch.busy_since.store(0, Ordering::Release);
        watch.iterations.record_unregistered(elapsed);
    }
}

/// Every metric the process has used, drained by the recorder.
pub(crate) struct Registry {
    timers: Mutex<Vec<&'static Timer>>,
    gauges: Mutex<Vec<&'static Gauge>>,
    counters: Mutex<Vec<&'static Counter>>,
    watches: Mutex<Vec<&'static LoopWatch>>,
}

impl Registry {
    pub(crate) const fn new() -> Self {
        Self {
            timers: Mutex::new(Vec::new()),
            gauges: Mutex::new(Vec::new()),
            counters: Mutex::new(Vec::new()),
            watches: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn add_timer(&self, timer: &'static Timer) {
        push(&self.timers, timer);
    }

    pub(crate) fn add_gauge(&self, gauge: &'static Gauge) {
        push(&self.gauges, gauge);
    }

    pub(crate) fn add_counter(&self, counter: &'static Counter) {
        push(&self.counters, counter);
    }

    pub(crate) fn add_watch(&self, watch: &'static LoopWatch) {
        push(&self.watches, watch);
    }

    fn timers(&self) -> Vec<&'static Timer> {
        snapshot(&self.timers)
    }

    fn gauges(&self) -> Vec<&'static Gauge> {
        snapshot(&self.gauges)
    }

    fn counters(&self) -> Vec<&'static Counter> {
        snapshot(&self.counters)
    }

    fn watches(&self) -> Vec<&'static LoopWatch> {
        snapshot(&self.watches)
    }

    #[cfg(test)]
    fn names(&self) -> Vec<&'static str> {
        let timers = self.timers().into_iter().map(|timer| timer.name);
        let gauges = self.gauges().into_iter().map(|gauge| gauge.name);
        let counters = self.counters().into_iter().map(|counter| counter.name);
        let watches = self.watches().into_iter().map(|watch| watch.name);
        timers
            .chain(gauges)
            .chain(counters)
            .chain(watches)
            .collect()
    }
}

fn push<T: ?Sized>(list: &Mutex<Vec<&'static T>>, item: &'static T) {
    if let Ok(mut list) = list.lock() {
        list.push(item);
    }
}

fn snapshot<T: ?Sized>(list: &Mutex<Vec<&'static T>>) -> Vec<&'static T> {
    list.lock().map(|list| list.clone()).unwrap_or_default()
}

static GLOBAL: Registry = Registry::new();

/// The recorder's latest sample of this process and the machine, for WARNs and the overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ContextSample {
    pub(crate) cpu_pct: f64,
    pub(crate) rss_mb: f64,
    pub(crate) load1: f64,
    pub(crate) sys_cpu_pct: f64,
}

static CONTEXT_SET: AtomicBool = AtomicBool::new(false);
static CONTEXT: [AtomicU64; 4] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

pub(crate) fn publish_context(sample: ContextSample) {
    let values = [
        sample.cpu_pct,
        sample.rss_mb,
        sample.load1,
        sample.sys_cpu_pct,
    ];
    for (slot, value) in CONTEXT.iter().zip(values) {
        slot.store(value.to_bits(), Ordering::Relaxed);
    }
    CONTEXT_SET.store(true, Ordering::Release);
}

/// None until the recorder's first sample (or when recording is off).
pub(crate) fn context() -> Option<ContextSample> {
    if !CONTEXT_SET.load(Ordering::Acquire) {
        return None;
    }
    let value = |index: usize| f64::from_bits(CONTEXT[index].load(Ordering::Relaxed));
    Some(ContextSample {
        cpu_pct: value(0),
        rss_mb: value(1),
        load1: value(2),
        sys_cpu_pct: value(3),
    })
}

/// A log field that reads `n/a` when the value is unknown.
pub(crate) struct Field(pub(crate) Option<f64>);

impl std::fmt::Display for Field {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(value) if value.is_finite() => {
                write!(formatter, "{}", (value * 10.0).round() / 10.0)
            }
            _ => formatter.write_str("n/a"),
        }
    }
}

pub(crate) fn global() -> &'static Registry {
    &GLOBAL
}

/// Microseconds on a process-wide monotonic clock; never 0, so 0 can mean "idle".
pub(crate) fn now_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    u64::try_from(epoch.elapsed().as_micros())
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}

#[cfg(test)]
#[path = "tests/metrics_test.rs"]
mod metrics_test;
