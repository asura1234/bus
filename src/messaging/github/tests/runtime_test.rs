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
                reason: "not logged in",
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
