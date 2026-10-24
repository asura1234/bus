//! Real submission time; fake providers can explicitly pause it in unit tests.
use std::time::{Duration, Instant};

#[cfg(test)]
pub(crate) const NATIVE_TEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
pub(crate) struct SubmissionClock {
    #[cfg(test)]
    state: std::sync::Arc<(std::sync::Mutex<TestTime>, std::sync::Condvar)>,
}

#[cfg(test)]
#[derive(Default)]
struct TestTime {
    now: Option<Instant>,
    generation: u64,
    enters: usize,
    draft_waits: usize,
    checked_generation: Option<u64>,
}

impl SubmissionClock {
    pub(crate) fn now(&self) -> Instant {
        #[cfg(test)]
        if let Some(now) = self.state.0.lock().unwrap().now {
            return now;
        }
        Instant::now()
    }

    pub(crate) fn wait_timeout(&self, timeout: Duration) -> Duration {
        #[cfg(test)]
        if self.state.0.lock().unwrap().now.is_some() {
            return NATIVE_TEST_TIMEOUT;
        }
        timeout
    }

    pub(super) fn remaining(&self, deadline: Instant) -> Duration {
        self.wait_timeout(deadline.saturating_duration_since(self.now()))
    }

    pub(super) fn generation(&self) -> u64 {
        #[cfg(test)]
        return self.state.0.lock().unwrap().generation;
        #[cfg(not(test))]
        0
    }

    pub(super) fn sleep(&self, duration: Duration, generation: u64) {
        #[cfg(test)]
        {
            let state = self.state.0.lock().unwrap();
            if state.now.is_some() {
                let (state, _) = self
                    .state
                    .1
                    .wait_timeout_while(state, NATIVE_TEST_TIMEOUT, |state| {
                        state.generation == generation
                    })
                    .unwrap();
                assert_ne!(
                    state.generation, generation,
                    "fake provider made no progress"
                );
                return;
            }
        }
        let _ = generation;
        std::thread::sleep(duration);
    }
}

#[cfg(test)]
impl SubmissionClock {
    pub(crate) fn pause(&self) {
        let mut state = self.state.0.lock().unwrap();
        if state.now.is_none() {
            state.now = Some(Instant::now());
            state.generation += 1;
        }
        self.state.1.notify_all();
    }

    pub(crate) fn resume(&self) {
        let mut state = self.state.0.lock().unwrap();
        state.now = None;
        state.generation += 1;
        self.state.1.notify_all();
    }

    pub(crate) fn advance(&self, by: Duration) -> u64 {
        let mut state = self.state.0.lock().unwrap();
        state.now = Some(state.now.expect("pause the fake clock first") + by);
        state.generation += 1;
        self.state.1.notify_all();
        state.generation
    }

    pub(crate) fn rendered(&self) {
        let mut state = self.state.0.lock().unwrap();
        if state.now.is_some() {
            state.generation += 1;
            self.state.1.notify_all();
        }
    }

    pub(super) fn entered(&self) {
        let mut state = self.state.0.lock().unwrap();
        state.enters += 1;
        self.state.1.notify_all();
    }

    pub(super) fn waiting_for_draft(&self) {
        let mut state = self.state.0.lock().unwrap();
        state.draft_waits += 1;
        self.state.1.notify_all();
    }

    pub(super) fn submission_checked(&self, generation: u64) {
        let mut state = self.state.0.lock().unwrap();
        state.checked_generation = Some(
            state
                .checked_generation
                .map_or(generation, |seen| seen.max(generation)),
        );
        self.state.1.notify_all();
    }

    fn wait_for(&self, ready: impl Fn(&TestTime) -> bool, message: &str) {
        let state = self.state.0.lock().unwrap();
        assert!(state.now.is_some(), "pause the fake clock first");
        let (state, _) = self
            .state
            .1
            .wait_timeout_while(state, NATIVE_TEST_TIMEOUT, |state| !ready(state))
            .unwrap();
        assert!(ready(&state), "{message}");
    }

    pub(crate) fn wait_for_enters(&self, count: usize) {
        self.wait_for(
            |state| state.enters >= count,
            "native Enter was not acknowledged",
        );
    }

    pub(crate) fn wait_for_draft_wait(&self) {
        self.wait_for(
            |state| state.draft_waits > 0,
            "writer did not wait for a rendered draft",
        );
    }

    pub(crate) fn wait_for_submission_check(&self, generation: u64) {
        self.wait_for(
            |state| {
                state
                    .checked_generation
                    .is_some_and(|checked| checked >= generation)
            },
            "writer did not inspect the advanced clock",
        );
    }
}
