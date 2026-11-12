//! Small screen facts published by the detector; list polling never reads VT cells.
use super::TerminalRuntime;
use crate::agents::AgentKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// Text comparisons are bounded separately from full metadata classification.
const FAST_REFRESH: std::time::Duration = std::time::Duration::from_millis(50);
const PARSE_INTERVAL: std::time::Duration = super::detection_policy::TICK_IDENTIFIED;

pub(super) struct ScreenDetectionCache {
    content_seq: Arc<AtomicU64>,
    latest: Mutex<Option<ScreenDetection>>,
    refresh: tokio::sync::Notify,
    last_observation: Mutex<Option<std::time::Instant>>,
    last_classification: Mutex<Option<std::time::Instant>>,
    #[cfg(test)]
    classifications: AtomicU64,
}

impl ScreenDetectionCache {
    pub(super) fn needs_refresh(&self, agent: Option<AgentKind>) -> bool {
        let revision = self.content_seq.load(Ordering::Acquire);
        !self
            .latest
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|snapshot| snapshot.agent == agent && snapshot.revision == revision)
    }

    /// Reuse the detector's active-screen text and parse outside the publication
    /// lock; list reads copy only the completed facts.
    pub(super) fn observe(
        &self,
        terminal: &crate::terminal::emulator::PaneTerminal,
        agent: Option<AgentKind>,
        process_exited: bool,
    ) -> String {
        *self
            .last_observation
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(std::time::Instant::now());
        let revision = self.content_seq.load(Ordering::Acquire);
        let content = terminal.detection_text();
        let coherent = revision.is_multiple_of(2)
            && revision == self.content_seq.load(Ordering::Acquire)
            && !terminal.synchronized_output_active();
        if agent.is_none() || process_exited {
            *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = None;
        } else if coherent {
            let previous = self
                .latest
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let snapshot = match previous {
                Some(mut previous)
                    if previous.agent == agent && previous.text.as_ref() == content =>
                {
                    // Identical repaints advance coherence without rerunning classifiers.
                    previous.revision = revision;
                    Some(previous)
                }
                _ => self.classify(agent, revision, &content),
            };
            if let Some(snapshot) =
                snapshot.filter(|_| revision == self.content_seq.load(Ordering::Acquire))
            {
                *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = Some(snapshot);
            }
        }
        content
    }

    pub(super) fn notify_changed(&self) {
        self.refresh.notify_one();
    }

    fn refresh_delay(&self, agent: Option<AgentKind>) -> Option<std::time::Duration> {
        if agent.is_none()
            || !self.content_seq.load(Ordering::Acquire).is_multiple_of(2)
            || !self.needs_refresh(agent)
        {
            return None;
        }
        let last = *self
            .last_observation
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        Some(last.map_or(std::time::Duration::ZERO, |at| {
            FAST_REFRESH.saturating_sub(at.elapsed())
        }))
    }

    pub(super) async fn wait_until(
        &self,
        terminal: &crate::terminal::emulator::PaneTerminal,
        agent: Option<AgentKind>,
        delay: std::time::Duration,
    ) {
        let tick = tokio::time::sleep(delay);
        tokio::pin!(tick);
        let mut pending = self
            .refresh_delay(agent)
            .map(|delay| tokio::time::Instant::now() + delay);
        loop {
            tokio::select! {
                biased;
                _ = &mut tick => return,
                _ = async {
                    if let Some(at) = pending {
                        tokio::time::sleep_until(at).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {},
                _ = self.refresh.notified(), if pending.is_none() => {},
            }
            self.observe_if_stale(terminal, agent);
            pending = self
                .refresh_delay(agent)
                .map(|delay| tokio::time::Instant::now() + delay);
        }
    }

    fn classify(
        &self,
        agent: Option<AgentKind>,
        revision: u64,
        text: &str,
    ) -> Option<ScreenDetection> {
        let mut last = self
            .last_classification
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if last
            .as_ref()
            .is_some_and(|at| at.elapsed() < PARSE_INTERVAL)
        {
            return None;
        }
        *last = Some(std::time::Instant::now());
        drop(last);
        #[cfg(test)]
        self.classifications.fetch_add(1, Ordering::Relaxed);
        Some(ScreenDetection::from_text(agent, revision, text))
    }

    #[cfg(test)]
    pub(super) fn prepare_test_tick(&self) {
        // Scripted fixture ticks advance independently of real wall-clock time.
        *self.last_classification.lock().unwrap() = None;
    }

    pub(super) fn observe_if_stale(
        &self,
        terminal: &crate::terminal::emulator::PaneTerminal,
        agent: Option<AgentKind>,
    ) {
        if self
            .refresh_delay(agent)
            .is_some_and(|delay| delay.is_zero())
        {
            self.observe(terminal, agent, false);
        }
    }

    pub(super) fn new(content_seq: Arc<AtomicU64>) -> Self {
        Self {
            content_seq,
            latest: Mutex::new(None),
            refresh: tokio::sync::Notify::new(),
            last_observation: Mutex::new(None),
            last_classification: Mutex::new(None),
            #[cfg(test)]
            classifications: AtomicU64::new(0),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ScreenDetection {
    text: Arc<str>,
    agent: Option<AgentKind>,
    revision: u64,
    pub(crate) compaction_banner: bool,
    pub(crate) provider_quota: bool,
    dialog: Option<String>,
    working_dialog: Option<String>,
}

impl ScreenDetection {
    pub(super) fn from_text(agent: Option<AgentKind>, revision: u64, text: &str) -> Self {
        Self {
            text: Arc::from(text),
            agent,
            revision,
            compaction_banner: crate::agents::compaction_banner::visible(
                agent.map(crate::agents::agent_label),
                text,
            ),
            provider_quota: agent == Some(AgentKind::Cursor)
                && crate::agents::providers::cursor::usage::is_restored_monthly_usage_limit(text),
            dialog: crate::terminal::driver::blocker_from_text(text, false),
            working_dialog: crate::terminal::driver::blocker_from_text(text, true),
        }
    }

    pub(crate) fn dialog(&self, working: bool) -> Option<String> {
        if working {
            self.working_dialog.clone()
        } else {
            self.dialog.clone()
        }
    }
}

impl TerminalRuntime {
    pub(crate) fn cached_screen_detection(
        &self,
        agent: Option<AgentKind>,
    ) -> Option<ScreenDetection> {
        let before = self.content_seq();
        // Publication rejects synchronized output. Every PTY write advances
        // content_seq, so a matching even revision rejects later partial frames
        // without acquiring the emulator's observation lock.
        if !before.is_multiple_of(2) {
            return None;
        }
        // Never wait for the detector, including during publication.
        let snapshot = self.screen_detection.latest.try_lock().ok()?.clone()?;
        (snapshot.agent == agent && snapshot.revision == before && before == self.content_seq())
            .then_some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[tokio::test]
    async fn cached_detection_does_not_wait_for_terminal_observation() {
        let runtime = TerminalRuntime::test_with_screen_bytes(80, 24, b"ready");
        runtime
            .screen_detection
            .observe(&runtime.terminal, Some(AgentKind::Claude), false);
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let terminal = runtime.terminal.clone();
        let holder = std::thread::spawn(move || {
            let _guard = terminal.ghostty.core.lock().unwrap();
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        held_rx.recv().unwrap();
        let (result_tx, result_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            result_tx
                .send(
                    runtime
                        .cached_screen_detection(Some(AgentKind::Claude))
                        .is_some(),
                )
                .unwrap();
        });
        let while_observation_held = result_rx.recv_timeout(Duration::from_secs(1));
        // Always release and join, including on the red path.
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        reader.join().unwrap();
        assert_eq!(
            while_observation_held,
            Ok(true),
            "a cached list read must not acquire the terminal observation lock"
        );
    }
    #[tokio::test]
    async fn cached_detection_rejects_a_redraw_started_after_publication() {
        use super::super::read::{pty_read_callback, PtyReadContext};
        use crate::utils::render::signal::RenderSignal;
        use std::sync::atomic::Ordering;
        use std::time::Instant;
        use tokio::sync::Notify;
        let runtime = TerminalRuntime::test_with_screen_bytes(80, 24, b"ready");
        runtime
            .screen_detection
            .observe(&runtime.terminal, Some(AgentKind::Claude), false);
        assert!(runtime
            .cached_screen_detection(Some(AgentKind::Claude))
            .is_some());
        let (response_writer, _responses) = tokio::sync::mpsc::channel(8);
        let (events, _events) = tokio::sync::mpsc::channel(8);
        let mut write = pty_read_callback(PtyReadContext {
            pane_id: runtime.pane_id,
            terminal: runtime.terminal.clone(),
            response_writer,
            render_notify: Arc::new(Notify::new()),
            render_dirty: Arc::new(RenderSignal::new()),
            content_seq: runtime.content_seq.clone(),
            content_write_lock: runtime.content_write_lock.clone(),
            detection_content_seq: runtime.detection_content_seq.clone(),
            screen_detection: runtime.screen_detection.clone(),
            child_pid: runtime.child_pid.clone(),
            events,
            reported_cwd: runtime.reported_cwd.clone(),
            compression_wake: runtime.compression.notifier(),
            rt: tokio::runtime::Handle::current(),
            agent_detection: super::super::AgentDetection::Enabled,
        });
        // A real PTY callback marks its revision odd before waiting on VT.
        // Hold VT so the cached reader deterministically intersects that write.
        let core = runtime.terminal.ghostty.core.lock().unwrap();
        let writer = std::thread::spawn(move || write(b"\x1b[?2026hpartial redraw"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while runtime
            .content_seq
            .load(Ordering::Acquire)
            .is_multiple_of(2)
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        let write_in_flight = !runtime.content_seq().is_multiple_of(2);
        let rejected_in_flight = write_in_flight
            && runtime
                .cached_screen_detection(Some(AgentKind::Claude))
                .is_none();
        drop(core);
        writer.join().unwrap();
        assert!(
            rejected_in_flight,
            "a PTY write must invalidate a previously published frame before VT changes"
        );
        assert!(runtime.synchronized_output_active());
        assert!(
            runtime
                .cached_screen_detection(Some(AgentKind::Claude))
                .is_none(),
            "an even revision during a partial redraw cannot revive the old snapshot"
        );
        runtime
            .screen_detection
            .observe(&runtime.terminal, Some(AgentKind::Claude), false);
        assert!(
            runtime
                .cached_screen_detection(Some(AgentKind::Claude))
                .is_none(),
            "a partial frame cannot publish new metadata"
        );
        runtime.test_process_pty_bytes(b"\x1b[?2026l");
        runtime.screen_detection.prepare_test_tick();
        runtime
            .screen_detection
            .observe(&runtime.terminal, Some(AgentKind::Claude), false);
        assert!(runtime
            .cached_screen_detection(Some(AgentKind::Claude))
            .is_some());
    }
    #[tokio::test]
    async fn pty_refresh_publishes_during_redraws_without_starving_scan_deadline() {
        use super::super::read::{pty_read_callback, PtyReadContext};
        use crate::utils::render::signal::RenderSignal;
        use tokio::sync::Notify;
        let runtime = TerminalRuntime::test_with_screen_bytes(100, 30, b"");
        let (response_writer, _responses) = tokio::sync::mpsc::channel(8);
        let (events, _events) = tokio::sync::mpsc::channel(8);
        let mut write = pty_read_callback(PtyReadContext {
            pane_id: runtime.pane_id,
            terminal: runtime.terminal.clone(),
            response_writer,
            render_notify: Arc::new(Notify::new()),
            render_dirty: Arc::new(RenderSignal::new()),
            content_seq: runtime.content_seq.clone(),
            content_write_lock: runtime.content_write_lock.clone(),
            detection_content_seq: runtime.detection_content_seq.clone(),
            screen_detection: runtime.screen_detection.clone(),
            child_pid: runtime.child_pid.clone(),
            events,
            reported_cwd: runtime.reported_cwd.clone(),
            compression_wake: runtime.compression.notifier(),
            rt: tokio::runtime::Handle::current(),
            agent_detection: super::super::AgentDetection::Enabled,
        });
        let cache = runtime.screen_detection.clone();
        let watcher_cache = cache.clone();
        let terminal = runtime.terminal.clone();
        let waiting = tokio::spawn(async move {
            watcher_cache
                .wait_until(&terminal, Some(AgentKind::Grok), Duration::from_secs(2))
                .await;
        });
        tokio::task::yield_now().await;
        let frame = b"\x1b[?2026h\x1b[2J\x1b[HContext compacted: 209.9k -> 37.8k tokens (131.2s)\r\n\x1b[?2026l";
        let sample_started = std::time::Instant::now();
        write(frame);
        let facts = tokio::time::timeout(Duration::from_millis(600), async {
            loop {
                if let Some(facts) = runtime.cached_screen_detection(Some(AgentKind::Grok)) {
                    break facts;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        if facts.is_err() {
            waiting.abort();
            let _ = waiting.await;
            panic!("a completed PTY frame must publish metadata before the periodic scan");
        }
        assert!(facts.unwrap().compaction_banner);
        // A final changed frame must publish after the coalescing delay even
        // when the producer sends no more bytes to wake the detector.
        write(b"\x1b[?2026h\x1b[2J\x1b[Hready\r\n\x1b[?2026l");
        let trailing = tokio::time::timeout(Duration::from_millis(600), async {
            loop {
                if let Some(facts) = runtime.cached_screen_detection(Some(AgentKind::Grok)) {
                    break facts;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        if trailing.is_err() {
            waiting.abort();
            let _ = waiting.await;
            panic!("coalescing must publish the final frame without another PTY write");
        }
        assert!(!trailing.unwrap().compaction_banner);
        let started = tokio::time::Instant::now();
        let mut frame_number = 0;
        while started.elapsed() < Duration::from_millis(2200) {
            let changing = format!("\x1b[?2026h\x1b[2J\x1b[Hframe {frame_number}\r\n\x1b[?2026l");
            write(changing.as_bytes());
            frame_number += 1;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let deadline_reached = waiting.is_finished();
        waiting.abort();
        let _ = waiting.await;
        assert!(
            deadline_reached,
            "redraw notifications must not reset the lifecycle/process scan deadline"
        );
        let parses = cache.classifications.load(Ordering::Relaxed);
        let allowed = sample_started.elapsed().as_millis() / PARSE_INTERVAL.as_millis() + 2;
        assert!(
            u128::from(parses) <= allowed,
            "changing PTY output exceeded the detector parse budget: {parses} > {allowed}"
        );
    }
}
