use super::*;
use std::time::Duration;

#[test]
fn timer_window_reports_count_average_max_and_slow_buckets_then_resets() {
    static TIMER: Timer = Timer::new("test.timer.window");
    TIMER.record(Duration::from_micros(500));
    TIMER.record(Duration::from_millis(20));
    TIMER.record(Duration::from_millis(150));
    TIMER.record(Duration::from_millis(1_200));

    let window = TIMER.take().expect("recorded samples");
    assert_eq!(window.count, 4);
    assert_eq!(window.total_us, 500 + 20_000 + 150_000 + 1_200_000);
    assert_eq!(window.max_us, 1_200_000);
    assert_eq!(window.over_16ms, 3);
    assert_eq!(window.over_100ms, 2);
    assert_eq!(window.over_1s, 1);
    assert!(TIMER.take().is_none(), "a taken window starts empty");
}

#[test]
fn timer_guard_records_its_scope() {
    static TIMER: Timer = Timer::new("test.timer.guard");
    {
        let _guard = TIMER.start();
        std::thread::sleep(Duration::from_millis(2));
    }
    let window = TIMER.take().expect("guard recorded");
    assert_eq!(window.count, 1);
    assert!(window.max_us >= 2_000, "{window:?}");
}

#[test]
fn gauge_window_keeps_max_and_last_value() {
    static GAUGE: Gauge = Gauge::new("test.gauge");
    assert!(GAUGE.take().is_none());
    GAUGE.observe(3);
    GAUGE.observe(9);
    GAUGE.observe(1);
    assert_eq!(GAUGE.take(), Some(GaugeWindow { max: 9, last: 1 }));
    assert!(GAUGE.take().is_none());
}

#[test]
fn counter_window_sums_and_resets() {
    static COUNTER: Counter = Counter::new("test.counter");
    COUNTER.add(0);
    assert_eq!(COUNTER.take(), None, "adding zero is not activity");
    COUNTER.add(4096);
    COUNTER.add(10);
    assert_eq!(COUNTER.take(), Some(4106));
    assert_eq!(COUNTER.take(), None);
}

#[test]
fn metrics_register_globally_on_first_use() {
    static TIMER: Timer = Timer::new("test.registry.timer");
    static WATCH: LoopWatch = LoopWatch::new("test.registry.loop");
    TIMER.record(Duration::from_micros(1));
    drop(WATCH.enter("stage"));
    let names = global().names();
    assert!(names.contains(&"test.registry.timer"), "{names:?}");
    assert!(names.contains(&"test.registry.loop"), "{names:?}");
    TIMER.record(Duration::from_micros(1));
    let again = global().names();
    assert_eq!(
        again
            .iter()
            .filter(|name| **name == "test.registry.timer")
            .count(),
        1,
        "registration happens once"
    );
}

#[test]
fn idle_or_fast_loop_reports_no_stall() {
    static WATCH: LoopWatch = LoopWatch::new("test.loop.fast");
    let threshold = Duration::from_millis(250);
    assert_eq!(WATCH.check(now_us(), threshold), None);
    let guard = WATCH.enter("tick");
    assert_eq!(WATCH.check(now_us(), threshold), None);
    drop(guard);
    assert_eq!(WATCH.check(now_us() + 10_000_000, threshold), None);
}

#[test]
fn blocked_loop_reports_one_stall_begin_with_its_stage_and_then_its_end() {
    static WATCH: LoopWatch = LoopWatch::new("test.loop.blocked");
    let threshold = Duration::from_millis(250);
    let guard = WATCH.enter("input");
    guard.stage("render");
    let later = now_us() + 400_000;

    let began = WATCH.check(later, threshold).expect("stall detected");
    assert_eq!(began.kind, StallKind::Began);
    assert_eq!(began.loop_name, "test.loop.blocked");
    assert_eq!(began.stage, "render");
    assert!(began.busy_ms >= 400, "{began:?}");
    assert_eq!(
        WATCH.check(later + 100_000, threshold),
        None,
        "an ongoing stall is reported once"
    );
    assert!(WATCH.ongoing_stall_ms(later + 100_000) >= Some(500));

    drop(guard);
    let ended = WATCH
        .check(now_us(), threshold)
        .expect("stall end reported");
    assert_eq!(ended.kind, StallKind::Ended);
    assert_eq!(ended.stage, "render");
    assert_eq!(WATCH.ongoing_stall_ms(now_us()), None);
    assert_eq!(WATCH.check(now_us() + 1_000_000, threshold), None);
}

#[test]
fn stall_end_survives_fast_iterations_between_watchdog_checks() {
    static WATCH: LoopWatch = LoopWatch::new("test.loop.burst");
    let threshold = Duration::from_millis(250);
    let guard = WATCH.enter("slow");
    std::thread::sleep(Duration::from_millis(5));
    assert!(WATCH.check(now_us() + 300_000, threshold).is_some());
    drop(guard);
    drop(WATCH.enter("fast"));
    drop(WATCH.enter("fast"));
    let ended = WATCH
        .check(now_us(), threshold)
        .expect("stall end reported");
    assert_eq!(ended.kind, StallKind::Ended);
    assert_eq!(ended.stage, "slow");
    assert!(ended.busy_ms >= 5, "{ended:?}");
}

#[test]
fn loop_iterations_feed_the_loop_timer() {
    static WATCH: LoopWatch = LoopWatch::new("test.loop.timer");
    drop(WATCH.enter("a"));
    drop(WATCH.enter("b"));
    let window = WATCH.iterations().take().expect("iterations recorded");
    assert_eq!(window.count, 2);
}

#[test]
fn slow_operations_are_recorded_and_logged_with_what_was_slow() {
    static TIMER: Timer = Timer::new("test.timer.op");
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        TIMER.record_op(Duration::from_millis(5), "agent.list");
        TIMER.record_op(Duration::from_millis(2_400), "agent.prompt_if_idle");
    });
    let text = capture.text();
    assert!(
        !text.contains("agent.list"),
        "fast ops are not logged: {text}"
    );
    assert!(text.contains("WARN"), "{text}");
    assert!(text.contains("perf.slow_op"), "{text}");
    assert!(text.contains("test.timer.op"), "{text}");
    assert!(text.contains("agent.prompt_if_idle"), "{text}");
    assert!(text.contains("elapsed_ms=2400"), "{text}");
    assert_eq!(TIMER.take().map(|window| window.count), Some(2));
}

#[test]
fn a_stall_ends_with_the_stage_that_blocked_not_the_last_one() {
    static WATCH: LoopWatch = LoopWatch::new("test.loop.relabel");
    let threshold = Duration::from_millis(250);
    let guard = WATCH.enter("tick");
    let began = WATCH.check(now_us() + 300_000, threshold).expect("stall");
    assert_eq!(began.stage, "tick");
    guard.stage("snapshot");
    drop(guard);
    let ended = WATCH.check(now_us(), threshold).expect("end");
    assert_eq!(ended.stage, "tick");
}

#[test]
fn slow_timers_warn_once_per_second_with_what_where_how_long_and_system_context() {
    static TIMER: Timer = Timer::new("test.timer.slow_frame");
    publish_context(ContextSample {
        cpu_pct: 87.5,
        rss_mb: 92.0,
        load1: 24.8,
        sys_cpu_pct: 99.0,
    });
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        TIMER.record(Duration::from_millis(150));
        TIMER.record(Duration::from_millis(400));
        TIMER.record(Duration::from_millis(5));
    });
    let text = capture.text();
    let warns: Vec<_> = text
        .lines()
        .filter(|line| line.contains("perf.slow_op"))
        .collect();
    assert_eq!(
        warns.len(),
        1,
        "rate limited to one WARN per second: {text}"
    );
    for field in [
        "WARN",
        "metric=\"test.timer.slow_frame\"",
        "elapsed_ms=150",
        "cpu_pct=87.5",
        "rss_mb=92",
        "load1=24.8",
        "sys_cpu_pct=99",
    ] {
        assert!(warns[0].contains(field), "{field} missing: {}", warns[0]);
    }
    assert_eq!(TIMER.take().map(|window| window.over_100ms), Some(2));
}
