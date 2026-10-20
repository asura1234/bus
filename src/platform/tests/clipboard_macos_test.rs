use super::*;
use std::cell::Cell;
use std::collections::VecDeque;
use std::time::Duration;

#[derive(Default)]
struct FakeClipboardRead {
    results: VecDeque<std::io::Result<Option<bool>>>,
    polls: usize,
    terminated_and_reaped: bool,
}

impl ClipboardReadProcess for FakeClipboardRead {
    fn poll(&mut self) -> std::io::Result<Option<bool>> {
        self.polls += 1;
        self.results.pop_front().unwrap_or(Ok(None))
    }

    fn terminate_and_reap(&mut self) {
        self.terminated_and_reaped = true;
    }
}

#[test]
fn clipboard_read_timeout_terminates_and_reaps_without_wall_clock_wait() {
    let mut child = FakeClipboardRead::default();
    let elapsed = Cell::new(Duration::ZERO);
    assert!(!wait_for_clipboard_read(
        &mut child,
        || elapsed.get(),
        || {
            elapsed.set(elapsed.get() + Duration::from_secs(1));
            assert!(
                elapsed.get() <= CLIPBOARD_READ_TIMEOUT,
                "clipboard read exceeded its deadline"
            );
        },
    ));
    assert_eq!(elapsed.get(), CLIPBOARD_READ_TIMEOUT);
    assert_eq!(child.polls, 2);
    assert!(child.terminated_and_reaped);
}

#[test]
fn clipboard_read_preserves_exit_status_before_deadline() {
    for success in [true, false] {
        let mut child = FakeClipboardRead {
            results: VecDeque::from([Ok(None), Ok(Some(success))]),
            ..Default::default()
        };
        let elapsed = Cell::new(Duration::ZERO);
        assert_eq!(
            wait_for_clipboard_read(
                &mut child,
                || elapsed.get(),
                || elapsed.set(elapsed.get() + Duration::from_millis(10)),
            ),
            success
        );
        assert_eq!(child.polls, 2);
        assert!(!child.terminated_and_reaped);
    }
}

#[test]
fn clipboard_read_poll_error_terminates_and_reaps() {
    let mut child = FakeClipboardRead {
        results: VecDeque::from([Err(std::io::Error::other("wait failed"))]),
        ..Default::default()
    };
    assert!(!wait_for_clipboard_read(
        &mut child,
        || Duration::ZERO,
        || panic!("must not wait after a poll error")
    ));
    assert!(child.terminated_and_reaped);
}
