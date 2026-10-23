use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
struct Fake {
    calls: AtomicUsize,
    release: Mutex<mpsc::Receiver<()>>,
    fail: bool,
}
impl Source for Fake {
    fn discover(&self, _: &[Interest]) -> BTreeMap<String, Vec<Scope>> {
        [(
            "o/r".into(),
            vec![
                Scope {
                    room: 7,
                    ..Default::default()
                },
                Scope {
                    room: 8,
                    ..Default::default()
                },
            ],
        )]
        .into()
    }
    fn fetch(&self, _: &Query) -> Result<Page, Failure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.release.lock().unwrap().recv().unwrap();
        if self.fail {
            Err(Failure {
                reason: "not logged in".into(),
                retry_at_ms: Some(900_000),
            })
        } else {
            Ok(Page::default())
        }
    }
}
fn drain(runtime: &mut Runtime, now: u64) -> Vec<Update> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let updates = runtime.tick(vec![], &BTreeMap::new(), now);
        if updates.iter().any(|u| matches!(u, Update::Repo { .. })) {
            return updates;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
#[test]
fn github_watch_tick_never_waits_for_source_and_shares_one_poll_across_rooms() {
    let (tx, rx) = mpsc::channel();
    let source = Arc::new(Fake {
        calls: AtomicUsize::new(0),
        release: Mutex::new(rx),
        fail: false,
    });
    let mut rt = Runtime::with_source(source.clone());
    let start = std::time::Instant::now();
    rt.tick(vec![], &BTreeMap::new(), 1);
    for _ in 0..100 {
        rt.tick(vec![], &BTreeMap::new(), 2);
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    tx.send(()).unwrap();
    drain(&mut rt, 3);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    rt.tick(vec![], &BTreeMap::new(), POLL_MS);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    rt.tick(vec![], &BTreeMap::new(), POLL_MS + 1);
    tx.send(()).unwrap();
    drain(&mut rt, POLL_MS + 2);
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
}
#[test]
fn github_watch_failure_backoff_honors_later_rate_reset() {
    let (tx, rx) = mpsc::channel();
    let source = Arc::new(Fake {
        calls: AtomicUsize::new(0),
        release: Mutex::new(rx),
        fail: true,
    });
    let mut rt = Runtime::with_source(source.clone());
    rt.tick(vec![], &BTreeMap::new(), 1);
    tx.send(()).unwrap();
    drain(&mut rt, 2);
    assert_eq!(rt.schedules["o/r"].next, 900_000);
    // A discovery round during backoff must not issue another gh call.
    rt.tick(vec![], &BTreeMap::new(), POLL_MS + 1);
    std::thread::sleep(std::time::Duration::from_millis(20));
    rt.tick(vec![], &BTreeMap::new(), POLL_MS + 2);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        retry_at(
            "X-RateLimit-Remaining: 0\nX-RateLimit-Reset: 1000\nRetry-After: 4",
            1
        ),
        Some(1_000_000)
    );
}

struct TwoRepos {
    fetched: Mutex<Vec<String>>,
}
impl Source for TwoRepos {
    fn discover(&self, _: &[Interest]) -> BTreeMap<String, Vec<Scope>> {
        ["a/one", "b/two"]
            .into_iter()
            .map(|repo| {
                (
                    repo.to_string(),
                    vec![Scope {
                        room: 7,
                        ..Default::default()
                    }],
                )
            })
            .collect()
    }
    fn fetch(&self, q: &Query) -> Result<Page, Failure> {
        self.fetched.lock().unwrap().push(q.repo.clone());
        // GraphQL rate limit is per account: one low-budget response covers all repos.
        Ok(Page {
            retry_at_ms: Some(100 * POLL_MS),
            ..Page::default()
        })
    }
}
fn finish_round(runtime: &mut Runtime, now: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while runtime.pending.is_some() || runtime.next_round <= now {
        runtime.tick(vec![], &BTreeMap::new(), now);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
#[test]
fn github_watch_account_rate_limit_reset_pauses_every_repository() {
    let source = Arc::new(TwoRepos {
        fetched: Mutex::new(Vec::new()),
    });
    let mut rt = Runtime::with_source(source.clone());
    finish_round(&mut rt, 1);
    let first = source.fetched.lock().unwrap().len();
    finish_round(&mut rt, POLL_MS + 1);
    let fetched = source.fetched.lock().unwrap().clone();
    assert_eq!(
        fetched.len(),
        first,
        "polled again before the account-wide rate-limit reset: {fetched:?}"
    );
}

#[test]
fn github_watch_failed_gh_response_preserves_bounded_network_and_http_errors() {
    let query = Query {
        repo: "o/r".into(),
        watermark: Watermark::default(),
        now_ms: 1,
    };
    let failure = parse_response(
        &query,
        false,
        "",
        "error connecting to api.github.com\nlookup api.github.com: no such host",
    )
    .unwrap_err();
    assert!(
        failure
            .reason
            .contains("lookup api.github.com: no such host"),
        "lost gh stderr: {failure:?}"
    );
    let failure = parse_response(&query, false, "HTTP/2.0 503 Service Unavailable\r\nRetry-After: 4\r\n\r\n{\"message\":\"upstream temporarily unavailable\"}", "").unwrap_err();
    assert!(
        failure.reason.contains("503 Service Unavailable"),
        "lost HTTP status: {failure:?}"
    );
    assert!(
        failure.reason.contains("upstream temporarily unavailable"),
        "lost HTTP body: {failure:?}"
    );
    assert_eq!(failure.retry_at_ms, Some(4001));
    let failure = parse_response(&query, false, "", &"故障".repeat(100_000)).unwrap_err();
    assert!(
        failure.reason.chars().count() <= 512,
        "unbounded diagnostic"
    );
    assert!(
        failure.reason.contains("故障"),
        "real error replaced by generic text"
    );
}

#[test]
fn github_watch_bounded_error_reaches_the_warning_and_unavailable_notice() {
    // Multiple dispatchers keep this shared callsite's interest independent
    // from concurrently running test threads without a default subscriber.
    let _registration = tracing::Dispatch::new(tracing_subscriber::registry());
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let logs = Arc::new(Mutex::new(Vec::new()));
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || Capture(writer.clone()))
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let query = Query {
        repo: "o/r".into(),
        watermark: Watermark::default(),
        now_ms: 1,
    };
    let failure = parse_response(
        &query,
        false,
        "",
        &format!("DNS lookup failed\n{}", "故障".repeat(100_000)),
    )
    .unwrap_err();
    assert_eq!(failure.reason.chars().count(), 512);
    assert!(failure.reason.ends_with('…'));
    assert!(!failure.reason.contains('\n'));
    let mut state = crate::messaging::model::BusState::new();
    let room = state.create_room("watch").unwrap();
    let update = Update::Repo {
        repo: query.repo,
        scopes: vec![Scope {
            room: room.0,
            ..Default::default()
        }],
        result: Err(failure.clone()),
        at_ms: 1,
    };
    let (tx, rx) = mpsc::channel();
    tx.send(update.clone()).unwrap();
    drop(tx);
    let mut runtime = Runtime {
        pending: Some(rx),
        ..Runtime::default()
    };
    runtime.tick(Vec::new(), &BTreeMap::new(), 1);
    let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains(&failure.reason),
        "warning lost the real bounded error: {logs}"
    );
    assert!(logs.len() < 2500, "unbounded warning row");
    for now in 1..=3 {
        state.apply_github(&update, now).unwrap();
    }
    let notice = state.notices(room).next().unwrap();
    assert!(
        notice.text.contains(&failure.reason),
        "alert lost the real bounded error"
    );
    assert!(notice.text.chars().count() < 750);
}

#[test]
fn github_watch_graphql_and_invalid_json_errors_keep_the_real_response_detail() {
    let query = Query {
        repo: "o/r".into(),
        watermark: Watermark::default(),
        now_ms: 1,
    };
    let failure = parse_response(
        &query,
        true,
        "{\"errors\":[{\"message\":\"fixture rate limit exceeded\"}]}",
        "",
    )
    .unwrap_err();
    assert!(failure.reason.contains("fixture rate limit exceeded"));
    let failure =
        parse_response(&query, true, "<html>fixture gateway failure</html>", "").unwrap_err();
    assert!(failure.reason.contains("fixture gateway failure"));
    let failure = parse_response(
        &query,
        false,
        "",
        "gh auth login: fixture credentials rejected",
    )
    .unwrap_err();
    assert!(failure.reason.contains("fixture credentials rejected"));
}
