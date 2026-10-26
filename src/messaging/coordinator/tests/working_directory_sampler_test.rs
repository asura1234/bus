use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Barrier,
};
use std::time::Duration;

fn state() -> (BusState, AgentId) {
    let mut state = BusState::default();
    let room = state.create_room("directory checks").unwrap();
    let id = state
        .create_agent(
            room,
            "worker",
            crate::messaging::model::Provider::Codex,
            "/query".into(),
            None,
        )
        .unwrap();
    (state, id)
}

#[test]
fn blocked_filesystem_query_does_not_hold_polling_or_spawn_more_queries() {
    let (mut state, _) = state();
    let release = Arc::new(Barrier::new(2));
    let query_release = release.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let query_calls = calls.clone();
    let (entered, started) = mpsc::channel();
    let (finished, returned) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut checks = DirectoryChecks {
            source: Arc::new(move |_| {
                query_calls.fetch_add(1, Ordering::SeqCst);
                entered.send(()).unwrap();
                query_release.wait();
                Ok(true)
            }),
            pending: None,
        };
        for _ in 0..100 {
            checks.poll(&mut state).unwrap();
        }
        finished.send(()).unwrap();
        // Hold the receiver until the blocked query is released.
        checks
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let completed_while_blocked = returned.recv_timeout(Duration::from_secs(5));
    let queries_while_blocked = calls.load(Ordering::SeqCst);
    release.wait();
    let _checks = thread.join().unwrap();
    assert!(
        completed_while_blocked.is_ok(),
        "polling waited for the filesystem query"
    );
    assert_eq!(
        queries_while_blocked, 1,
        "a stuck query must not create more workers"
    );
}

#[test]
fn completed_directory_check_does_not_apply_to_a_changed_path() {
    let (mut state, id) = state();
    let (sender, receiver) = mpsc::sync_channel(1);
    sender
        .send(vec![(id, PathBuf::from("/query"), Ok(false))])
        .unwrap();
    let mut checks = DirectoryChecks {
        source: Arc::new(|_| Ok(true)),
        pending: Some(receiver),
    };
    state
        .update_agent_runtime_metadata(id, "/new-worktree".into(), None)
        .unwrap();
    checks.poll(&mut state).unwrap();
    assert!(state.agent(id).unwrap().working_directory_error.is_none());
    assert!(state.agent(id).unwrap().actionable_error.is_none());
}
