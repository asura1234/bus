use super::*;
use tracing_subscriber::prelude::*;

fn emit_error() {
    tracing::error!(
        event = "bus.test.failure",
        secret = "must stay in logs",
        "private error detail"
    );
}

#[test]
fn error_pipe_captures_metadata_only_and_filters_warn_noise_and_disabled_sessions() {
    let root = crate::utils::test_temp::unique_temp_path("bus-error-pipe-capture");
    crate::platform::fs::private_dir(&root).unwrap();
    let log = root.join("client.log");
    let mut runtime = Runtime::default();
    runtime.configure(&root, Some("orch"));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer::new(log.clone()));
    tracing::subscriber::with_default(subscriber, || {
        emit_error();
        tracing::warn!(event = "bus.clipboard.failed", error = "detail", "broken");
        tracing::warn!(event = "perf.stall", "slow");
        tracing::warn!(event = "api.wait.timeout", "slow");
        tracing::warn!(event = "bus.form.validation_failed", "expected");
        tracing::info!(event = "bus.info", "fine");
        let _guard = suppress(true);
        emit_error();
    });
    let records = drain(&root).unwrap();
    assert_eq!(
        records.len(),
        2,
        "only failures, no suppressed report-send error"
    );
    assert_eq!(records[0].event, "bus.test.failure");
    assert_eq!(records[0].log, log);
    assert!(records[0].location.contains("tests.rs:"));
    let serialized = serde_json::to_string(&records).unwrap();
    assert!(!serialized.contains("must stay in logs"));
    assert!(!serialized.contains("private error detail"));
    runtime.configure(&root, None);
    tracing::subscriber::with_default(
        tracing_subscriber::registry().with(CaptureLayer::new(root.join("server.log"))),
        emit_error,
    );
    assert!(drain(&root).unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

fn record(location: &str) -> Record {
    Record {
        event: "bus.test.failure".into(),
        location: location.into(),
        log: "/logs/server.log".into(),
        generation: "test".into(),
    }
}

#[test]
fn error_pipe_counts_first_burst_then_summarizes_only_new_repeats_once_per_minute() {
    let mut batch = Batches::default();
    batch.add(record("a.rs:1"), 100);
    batch.add(record("a.rs:1"), 500);
    batch.add(record("a.rs:2"), 500);
    assert!(batch.notices(1099).is_empty());
    let first = batch.notices(1100);
    assert_eq!(
        first,
        ["Bus error: bus.test.failure (x2 since last notice). Logs: /logs/server.log"]
    );
    assert_eq!(
        batch.notices(1500).len(),
        1,
        "different location gets its own notice"
    );
    batch.add(record("a.rs:1"), 2000);
    assert!(batch.notices(61099).is_empty());
    assert_eq!(
        batch.notices(61100),
        ["Bus error: bus.test.failure (x1 since last notice). Logs: /logs/server.log"]
    );
    assert!(
        batch.notices(200000).is_empty(),
        "quiet errors never send again"
    );
}

#[test]
fn error_pipe_nested_suppression_restores_the_outer_state() {
    assert!(!is_suppressed());
    {
        let _outer = suppress(true);
        assert!(is_suppressed());
        {
            let _inner = suppress(false);
            assert!(is_suppressed());
        }
        assert!(is_suppressed());
    }
    assert!(!is_suppressed());
}

#[test]
fn error_pipe_report_rpc_scope_stays_suppressed_on_the_async_submission_thread() {
    let root = crate::utils::test_temp::unique_temp_path("bus-error-pipe-rpc");
    let mut runtime = Runtime::default();
    runtime.configure(&root, Some("lead"));
    let subscriber =
        tracing_subscriber::registry().with(CaptureLayer::new(root.join("server.log")));
    let dispatch = tracing::Dispatch::new(subscriber);
    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!(
            "native_submission",
            api_request_id = "bus-error-report:123:1"
        );
        let _entered = span.enter();
        let dispatch = dispatch.clone();
        let span = span.clone();
        std::thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                let _entered = span.enter();
                emit_error();
            })
        })
        .join()
        .unwrap();
    });
    tracing::dispatcher::with_default(&dispatch, emit_error);
    assert_eq!(
        drain(&root).unwrap().len(),
        1,
        "only the unrelated error is reportable"
    );
    std::fs::remove_dir_all(root).unwrap();
}
