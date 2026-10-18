use super::super::metrics::OutputMetrics;
use super::*;

#[test]
fn output_owner_registers_frame_patch_and_backpressure_metrics() {
    let (mut output, state, entered) = slow(false);
    output.frame(frame("A", 4), false, false).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    output.frame(frame("B", 4), false, false).unwrap();
    output.frame(frame("C", 4), false, false).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    assert!(output.handle.busy());
    output.feedback().unwrap();
    release(&state);
    output.handle.wait_idle();
    assert!(output
        .patch(
            &[PaneSurfacePatchRow {
                y: 0,
                x: 0,
                cells: frame("D", 1).cells,
            }],
            None,
            false,
        )
        .unwrap());
    output.finish().unwrap();
    let names = crate::utils::perf::global().names();
    for name in [
        "client.output.frame.encode",
        "client.output.frame.write",
        "client.output.frame.flush",
        "client.output.frame.encoded_bytes",
        "client.output.patch.encode",
        "client.output.patch.write",
        "client.output.patch.flush",
        "client.output.patch.encoded_bytes",
        "client.output.frames_coalesced",
        "client.output.pending_age_ms",
    ] {
        assert!(
            names.contains(&name),
            "missing {name}; registered: {names:?}"
        );
    }
}

#[derive(Default)]
struct Phases {
    write_open: bool,
    flush_open: bool,
    bytes: Vec<u8>,
}

struct PhaseWriter {
    state: Arc<(Mutex<Phases>, Condvar)>,
    write_entered: Option<mpsc::Sender<()>>,
    flush_entered: Option<mpsc::Sender<()>>,
}

struct OpenOnDrop(Arc<(Mutex<Phases>, Condvar)>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        let mut state = self.0 .0.lock().unwrap();
        state.write_open = true;
        state.flush_open = true;
        self.0 .1.notify_all();
    }
}

impl Write for PhaseWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(tx) = self.write_entered.take() {
            tx.send(()).unwrap();
        }
        let mut state = self.state.0.lock().unwrap();
        while !state.write_open {
            state = self.state.1.wait(state).unwrap();
        }
        let count = bytes.len().min(3);
        state.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(tx) = self.flush_entered.take() {
            tx.send(()).unwrap();
        }
        let mut state = self.state.0.lock().unwrap();
        while !state.flush_open {
            state = self.state.1.wait(state).unwrap();
        }
        Ok(())
    }
}

#[test]
fn frame_metrics_separate_blocked_write_from_flush_and_patch_metrics_count_partial_writes_once() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    let state = Arc::new((Mutex::new(Phases::default()), Condvar::new()));
    let (write_tx, write_rx) = mpsc::channel();
    let (flush_tx, flush_rx) = mpsc::channel();
    let mut output = TerminalOutput::with_metrics(
        PhaseWriter {
            state: state.clone(),
            write_entered: Some(write_tx),
            flush_entered: Some(flush_tx),
        },
        &METRICS,
    )
    .unwrap();
    let _release = OpenOnDrop(state.clone());
    let mut first = frame("A", 4);
    first.graphics = b"\x1b_Ga=d,d=A\x1b\\".to_vec();
    output.frame(first, false, false).unwrap();
    write_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(METRICS.frame.encode.take().unwrap().count, 1);
    let encoded_bytes = METRICS.frame.encoded_bytes.take().unwrap();
    assert!(METRICS.frame.write.take().is_none());
    assert!(METRICS.frame.flush.take().is_none());
    std::thread::sleep(Duration::from_millis(5));
    output.feedback().unwrap();
    assert!(METRICS.pending_age_ms.take().unwrap().last >= 5);
    state.0.lock().unwrap().write_open = true;
    state.1.notify_all();
    flush_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let write = METRICS.frame.write.take().unwrap();
    assert_eq!(write.count, 1, "partial writes form one frame write");
    assert!(write.max_us >= 5_000);
    assert_eq!(encoded_bytes, state.0.lock().unwrap().bytes.len() as u64);
    assert!(METRICS.frame.flush.take().is_none());
    std::thread::sleep(Duration::from_millis(5));
    state.0.lock().unwrap().flush_open = true;
    state.1.notify_all();
    output.handle.wait_idle();
    let flush = METRICS.frame.flush.take().unwrap();
    assert_eq!(flush.count, 1);
    assert!(flush.max_us >= 5_000);
    assert_eq!(METRICS.pending_age_ms.take().unwrap().last, 0);
    let before = state.0.lock().unwrap().bytes.len();
    output
        .patch(
            &[PaneSurfacePatchRow {
                y: 0,
                x: 0,
                cells: frame("D", 1).cells,
            }],
            None,
            false,
        )
        .unwrap();
    output.finish().unwrap();
    for timer in [
        &METRICS.patch.encode,
        &METRICS.patch.write,
        &METRICS.patch.flush,
    ] {
        assert_eq!(timer.take().unwrap().count, 1);
    }
    assert_eq!(
        METRICS.patch.encoded_bytes.take().unwrap(),
        (state.0.lock().unwrap().bytes.len() - before) as u64,
    );
    assert!(
        METRICS.frame.encode.take().is_none(),
        "fast patch stayed a patch"
    );
}

#[test]
fn coalescing_keeps_the_first_pending_age_and_failed_output_clears_it() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    let handle = Handle(Arc::new(Shared::new(&METRICS)));
    let mut output = TerminalOutput {
        handle: handle.clone(),
        worker: None,
        latest: None,
        sequence: 0,
        invalidate: false,
    };
    output.frame(frame("A", 4), false, false).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    for _ in 0..100 {
        output.frame(frame("B", 4), false, false).unwrap();
    }
    assert_eq!(METRICS.coalesced.take(), Some(100));
    assert!(METRICS.pending_age_ms.take().unwrap().last >= 5);
    let queued = handle.next().unwrap();
    output.feedback().unwrap();
    assert!(METRICS.pending_age_ms.take().unwrap().last >= 5);
    output.frame(frame("C", 4), false, false).unwrap();
    handle.complete(queued, Err(io::Error::other("numeric-metric-test-secret")));
    assert!(output.feedback().is_err());
    assert_eq!(METRICS.pending_age_ms.take().unwrap().last, 0);
    assert!(output.finish().is_err());
}

#[test]
fn failed_flush_records_each_attempt_without_committing_the_baseline() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    let state = Arc::new((
        Mutex::new(Capture {
            open: true,
            ..Capture::default()
        }),
        Condvar::new(),
    ));
    let mut output = TerminalOutput::with_metrics(
        SlowWriter {
            state: state.clone(),
            entered: None,
            fail_flush: true,
        },
        &METRICS,
    )
    .unwrap();
    output.frame(frame("A", 4), false, false).unwrap();
    assert!(output.finish().is_err());
    assert_eq!(output.handle.snapshot().2, 0);
    for timer in [
        &METRICS.frame.encode,
        &METRICS.frame.write,
        &METRICS.frame.flush,
    ] {
        assert_eq!(timer.take().unwrap().count, 1);
    }
    assert_eq!(
        METRICS.frame.encoded_bytes.take().unwrap(),
        state.0.lock().unwrap().bytes.len() as u64
    );
    assert_eq!(METRICS.pending_age_ms.take().unwrap().last, 0);
}

#[test]
fn a_coalesced_patch_fallback_records_a_frame_without_claiming_a_patch_write() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    let state = Arc::new((Mutex::new(Capture::default()), Condvar::new()));
    let (tx, rx) = mpsc::channel();
    let mut output = TerminalOutput::with_metrics(
        SlowWriter {
            state: state.clone(),
            entered: Some(tx),
            fail_flush: false,
        },
        &METRICS,
    )
    .unwrap();
    output.frame(frame("A", 4), false, false).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    output.frame(frame("B", 4), false, false).unwrap();
    output
        .patch(
            &[PaneSurfacePatchRow {
                y: 0,
                x: 0,
                cells: frame("C", 1).cells,
            }],
            None,
            false,
        )
        .unwrap();
    release(&state);
    output.finish().unwrap();
    for timer in [
        &METRICS.frame.encode,
        &METRICS.frame.write,
        &METRICS.frame.flush,
    ] {
        assert_eq!(timer.take().unwrap().count, 2);
    }
    assert!(METRICS.patch.encode.take().is_none());
    assert!(METRICS.patch.write.take().is_none());
    assert!(METRICS.patch.flush.take().is_none());
    assert!(METRICS.patch.encoded_bytes.take().is_none());
    assert_eq!(METRICS.coalesced.take(), Some(1));
    assert_eq!(
        METRICS.frame.encoded_bytes.take().unwrap(),
        state.0.lock().unwrap().bytes.len() as u64
    );
}

#[test]
fn failed_write_records_the_write_but_never_claims_a_flush() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    struct FailWrite;
    impl Write for FailWrite {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("numeric-metric-test-secret"))
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("a failed write must not flush");
        }
    }
    let mut output = TerminalOutput::with_metrics(FailWrite, &METRICS).unwrap();
    output.frame(frame("A", 4), false, false).unwrap();
    assert!(output.finish().is_err());
    assert_eq!(METRICS.frame.encode.take().unwrap().count, 1);
    assert_eq!(METRICS.frame.write.take().unwrap().count, 1);
    assert!(METRICS.frame.flush.take().is_none());
    assert!(METRICS.frame.encoded_bytes.take().unwrap() > 0);
    assert_eq!(output.handle.snapshot().2, 0);
    assert_eq!(METRICS.pending_age_ms.take().unwrap().last, 0);
}

#[test]
fn preencoded_terminal_frames_have_output_metrics_without_inventing_encoding_time() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    let state = Arc::new((
        Mutex::new(Capture {
            open: true,
            ..Capture::default()
        }),
        Condvar::new(),
    ));
    let mut output = TerminalOutput::with_metrics(
        SlowWriter {
            state: state.clone(),
            entered: None,
            fail_flush: false,
        },
        &METRICS,
    )
    .unwrap();
    let bytes = b"\x1b[31mprivate-terminal-frame\x1b[0m";
    output.terminal_bytes(bytes.to_vec()).unwrap();
    output.finish().unwrap();
    assert_eq!(state.0.lock().unwrap().bytes, bytes);
    assert_eq!(
        METRICS
            .frame
            .write
            .take()
            .expect("legacy frame write measured")
            .count,
        1
    );
    assert_eq!(METRICS.frame.flush.take().unwrap().count, 1);
    assert_eq!(METRICS.frame.encoded_bytes.take(), Some(bytes.len() as u64));
    assert!(
        METRICS.frame.encode.take().is_none(),
        "owner receives this frame already encoded"
    );
}

#[test]
fn terminal_effects_and_standalone_graphics_do_not_pollute_frame_metrics() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    let state = Arc::new((
        Mutex::new(Capture {
            open: true,
            ..Capture::default()
        }),
        Condvar::new(),
    ));
    let mut output = TerminalOutput::with_metrics(
        SlowWriter {
            state: state.clone(),
            entered: None,
            fail_flush: false,
        },
        &METRICS,
    )
    .unwrap();
    let effect = b"\x1b]0;private-terminal-title\x07";
    {
        let _route = output.route();
        let mut writer = stdout();
        writer.write_all(effect).unwrap();
        writer.flush().unwrap();
    }
    let graphics = b"\x1b_Ga=d,d=A\x1b\\";
    output.graphics(graphics).unwrap();
    output.finish().unwrap();
    assert_eq!(
        state.0.lock().unwrap().bytes,
        [effect.as_slice(), b"\x1b7", graphics, b"\x1b8"].concat()
    );
    assert!(METRICS.frame.encode.take().is_none());
    assert!(METRICS.frame.write.take().is_none());
    assert!(METRICS.frame.flush.take().is_none());
    assert!(METRICS.frame.encoded_bytes.take().is_none());
}

#[test]
fn a_frame_coalesced_behind_an_active_write_keeps_its_first_enqueue_age() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    let handle = Handle(Arc::new(Shared::new(&METRICS)));
    let mut output = TerminalOutput {
        handle: handle.clone(),
        worker: None,
        latest: None,
        sequence: 0,
        invalidate: false,
    };
    output.frame(frame("A", 4), false, false).unwrap();
    let active = handle.next().unwrap();
    output.frame(frame("B", 4), false, false).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    output.frame(frame("C", 4), false, false).unwrap();
    assert_eq!(METRICS.coalesced.take(), Some(1));
    METRICS.pending_age_ms.take();
    handle.complete(active, Ok(Some(false)));
    assert!(
        METRICS.pending_age_ms.take().unwrap().last >= 20,
        "after the active write completes, the queue front keeps the replaced frame's enqueue time"
    );
    let queued = handle.next().unwrap();
    handle.complete(queued, Ok(Some(false)));
    assert_eq!(METRICS.pending_age_ms.take().unwrap().last, 0);
}

#[test]
fn a_replacement_rejected_by_the_pending_limit_is_not_counted_as_coalesced() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    let mut output = TerminalOutput {
        handle: Handle(Arc::new(Shared::new(&METRICS))),
        worker: None,
        latest: None,
        sequence: 0,
        invalidate: false,
    };
    output
        .handle
        .push(Command::Bytes(vec![0; 60 * 1024 * 1024]))
        .unwrap();
    output.frame(frame("A", 4), false, false).unwrap();
    assert_eq!(output.handle.snapshot().0, 2);
    let mut large = frame("A", 4);
    large.cells[0].symbol = "x".repeat(5 * 1024 * 1024);
    assert!(output.frame(large, false, false).is_err());
    assert!(METRICS.coalesced.take().is_none());
}

#[test]
fn failed_patch_write_records_the_patch_write_but_never_claims_a_flush() {
    static METRICS: OutputMetrics = OutputMetrics::new();
    struct FailAfterFirstFrame {
        fail: Arc<std::sync::atomic::AtomicBool>,
        flushes_after_failure: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Write for FailAfterFirstFrame {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(io::Error::other("numeric-metric-test-secret"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                self.flushes_after_failure
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        }
    }
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flushes_after_failure = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut output = TerminalOutput::with_metrics(
        FailAfterFirstFrame {
            fail: fail.clone(),
            flushes_after_failure: flushes_after_failure.clone(),
        },
        &METRICS,
    )
    .unwrap();
    output.frame(frame("A", 4), false, false).unwrap();
    output.handle.wait_idle();
    let presented = output.handle.snapshot().2;
    fail.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(output
        .patch(
            &[PaneSurfacePatchRow {
                y: 0,
                x: 0,
                cells: frame("D", 1).cells,
            }],
            None,
            false,
        )
        .unwrap());
    assert!(output.finish().is_err());
    assert_eq!(METRICS.patch.encode.take().unwrap().count, 1);
    assert_eq!(METRICS.patch.write.take().unwrap().count, 1);
    assert!(METRICS.patch.flush.take().is_none());
    assert!(METRICS.patch.encoded_bytes.take().unwrap() > 0);
    assert_eq!(
        flushes_after_failure.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        output.handle.snapshot().2,
        presented,
        "failed patch is not presented"
    );
    assert_eq!(METRICS.pending_age_ms.take().unwrap().last, 0);
}
