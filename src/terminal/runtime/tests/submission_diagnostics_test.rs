use super::*;

#[derive(Clone)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);
impl tracing::Subscriber for LogCapture {
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }
    fn new_span(&self, attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        attributes.record(&mut Fields(self.0.clone()));
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        values.record(&mut Fields(self.0.clone()));
    }
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        event.record(&mut Fields(self.0.clone()));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
struct Fields(Arc<Mutex<Vec<u8>>>);
impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::io::Write;
        writeln!(self.0.lock().unwrap(), "{}={value:?}", field.name()).unwrap();
    }
}

#[tokio::test]
async fn submission_diagnostics_record_attempts_without_prompt_or_screen_content() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let captured = Arc::new(Mutex::new(Vec::new()));
    // tracing-core's single-dispatch fast path registers a new callsite
    // against the registering thread's default. A sibling without a subscriber
    // can therefore cache Interest::never for our summary. A second live
    // dispatch makes registration consider every subscriber instead.
    let _registration = tracing::Dispatch::new(tracing_subscriber::registry());
    let dispatch = tracing::Dispatch::new(LogCapture(captured.clone()));
    let _capture_guard = tracing::dispatcher::set_default(&dispatch);
    // A sibling submission can register this shared callsite without a
    // subscriber before the captured submission reaches its summary.
    std::thread::spawn(|| {
        diagnostics::Diagnostics::default().finish(&Ok(()), Instant::now());
    })
    .join()
    .unwrap();
    let completion = tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!(
            "native_submission",
            api_request_id = "diagnostic-api",
            provider = "claude",
            pane_id = "scratch-pane"
        );
        let _entered = span.enter();
        runtime
            .queue_observed_input_submission(
                Bytes::from_static(b"SECRET_DIAGNOSTIC_PROMPT"),
                Bytes::from_static(b"\r"),
                vec![Bytes::from_static(b"\x01")],
                Duration::ZERO,
                None,
                |screen| {
                    if screen.contains("SECRET_DIAGNOSTIC_PROMPT") {
                        InputObservation::Pending
                    } else {
                        InputObservation::Empty
                    }
                },
            )
            .unwrap()
    });
    assert_eq!(
        writes.recv().await.unwrap(),
        Bytes::from_static(b"SECRET_DIAGNOSTIC_PROMPT")
    );
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[HSECRET_DIAGNOSTIC_PROMPT");
    assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"\r"));
    clock.wait_for_enters(1);
    clock.advance(Duration::from_millis(250));
    assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"\r"));
    clock.wait_for_enters(2);
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
    completion
        .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
        .unwrap()
        .unwrap();
    let log = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    for fact in [
        "bus.terminal.submission.summary",
        "diagnostic-api",
        "scratch-pane",
        "enter_attempts=2",
        "enter_flushed=2",
        "owned_seen=true",
        "paste_flushed=true",
        "final_observation=Some(Empty)",
    ] {
        assert!(log.contains(fact), "missing diagnostic {fact}: {log}");
    }
    assert!(
        !log.contains("SECRET_DIAGNOSTIC_PROMPT"),
        "diagnostics leaked content: {log}"
    );
    assert!(
        !log.contains("screen="),
        "raw screen logging is forbidden: {log}"
    );
}
