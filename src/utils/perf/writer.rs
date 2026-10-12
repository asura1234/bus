//! The `bus-perf-writer` thread: appends perf records off the watchdog.
//!
//! The role file is shared by every process of a role and locked per append, so
//! a write can wait on another process. The watchdog only queues records and
//! counts the ones a full queue refuses: it never logs or touches a file. The
//! writer reports losses, once per overload episode, and on shutdown drains
//! what was queued within a deadline.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::utils::logging::RotatingFileState;

/// About four minutes of windows; stall records are rarer.
pub(super) const QUEUE_RECORDS: usize = 256;
/// An overload episode ends once the queue is empty and nothing was lost for
/// this long, so a writer that stays slower than the watchdog warns once.
pub(super) const QUIET_AFTER_LOSS: Duration = Duration::from_secs(1);
/// How long a normal exit waits for the writer to drain the queue.
pub(crate) const DRAIN_DEADLINE: Duration = Duration::from_secs(2);
/// How often an idle writer checks whether an overload episode ended.
const IDLE_CHECK: Duration = Duration::from_millis(250);

enum Message {
    Line(Vec<u8>),
    #[cfg(test)]
    Flush(mpsc::Sender<()>),
}

/// Counters shared by the watchdog and the writer.
#[derive(Default)]
pub(super) struct Counters {
    /// Records accepted into the queue and not yet written or failed.
    pub(super) queued: AtomicU64,
    /// Records the queue refused since the writer last accounted for them.
    pub(super) dropped: AtomicU64,
}

/// The watchdog's end: queuing never blocks, logs or does I/O.
pub(super) struct WriterHandle {
    role: &'static str,
    queue: Option<mpsc::SyncSender<Message>>,
    counters: Arc<Counters>,
    finished: mpsc::Receiver<()>,
}

impl WriterHandle {
    /// Starts the writer thread for `out`. It logs through the subscriber that
    /// is current here, which in production is the process's file logger.
    pub(super) fn spawn(
        role: &'static str,
        out: RotatingFileState,
        capacity: usize,
    ) -> std::io::Result<Self> {
        let (queue, received) = mpsc::sync_channel(capacity);
        let (done, finished) = mpsc::channel();
        let counters = Arc::new(Counters::default());
        let mut writer = PerfWriter::new(role, out, Arc::clone(&counters));
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        std::thread::Builder::new()
            .name("bus-perf-writer".into())
            .spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || writer.run(&received));
                let _ = done.send(());
            })?;
        Ok(Self {
            role,
            queue: Some(queue),
            counters,
            finished,
        })
    }

    /// Queues one record; a full queue or a stopped writer counts it as lost.
    pub(super) fn send(&self, line: Vec<u8>) {
        let Some(queue) = &self.queue else { return };
        // Counted before the send so the writer never sees it go negative.
        self.counters.queued.fetch_add(1, Ordering::AcqRel);
        if queue.try_send(Message::Line(line)).is_err() {
            self.counters.queued.fetch_sub(1, Ordering::AcqRel);
            self.counters.dropped.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Stops queuing and lets the writer drain, waiting at most `deadline`.
    /// Records still queued after it are reported as lost.
    pub(super) fn shutdown(&mut self, deadline: Duration) {
        let Some(queue) = self.queue.take() else {
            return;
        };
        drop(queue);
        if self.finished.recv_timeout(deadline).is_ok() {
            return;
        }
        let undrained = self.counters.queued.load(Ordering::Acquire);
        tracing::warn!(
            event = "perf.write_failed",
            role = self.role,
            undrained_records = undrained,
            deadline_ms = u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX),
            "perf records still queued at exit are lost"
        );
    }

    /// Waits until every record queued so far is written.
    #[cfg(test)]
    pub(super) fn flush(&self) {
        let Some(queue) = &self.queue else { return };
        let (done, wait) = mpsc::channel();
        if queue.send(Message::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }
}

impl Drop for WriterHandle {
    fn drop(&mut self) {
        self.shutdown(DRAIN_DEADLINE);
    }
}

/// The writer thread's state: the file and the loss accounting.
pub(super) struct PerfWriter {
    role: &'static str,
    out: RotatingFileState,
    counters: Arc<Counters>,
    /// Records lost in the current episode; `None` when nothing is being lost.
    episode: Option<Episode>,
}

struct Episode {
    lost: u64,
    last_loss: Instant,
}

impl PerfWriter {
    pub(super) fn new(role: &'static str, out: RotatingFileState, counters: Arc<Counters>) -> Self {
        Self {
            role,
            out,
            counters,
            episode: None,
        }
    }

    fn run(&mut self, received: &mpsc::Receiver<Message>) {
        loop {
            match received.recv_timeout(IDLE_CHECK) {
                Ok(Message::Line(line)) => {
                    self.write(&line, Instant::now());
                    self.counters.queued.fetch_sub(1, Ordering::AcqRel);
                }
                #[cfg(test)]
                Ok(Message::Flush(done)) => {
                    let _ = done.send(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            self.account(Instant::now());
        }
        // Drained: whatever this episode lost is final.
        self.end_episode();
    }

    /// Appends one record, counting a failure as a loss.
    pub(super) fn write(&mut self, line: &[u8], now: Instant) {
        if let Err(error) = self.out.append(line) {
            self.lose(1, now, &error.to_string());
        }
    }

    /// Takes the queue's refusals into the episode, and ends the episode once
    /// the queue is empty and nothing was lost for [`QUIET_AFTER_LOSS`].
    pub(super) fn account(&mut self, now: Instant) {
        let dropped = self.counters.dropped.swap(0, Ordering::AcqRel);
        if dropped > 0 {
            self.lose(dropped, now, "writer queue full");
        }
        let quiet = self.episode.as_ref().is_some_and(|episode| {
            now.saturating_duration_since(episode.last_loss) >= QUIET_AFTER_LOSS
        });
        if quiet && self.counters.queued.load(Ordering::Acquire) == 0 {
            self.end_episode();
        }
    }

    fn lose(&mut self, records: u64, now: Instant, cause: &str) {
        match &mut self.episode {
            Some(episode) => {
                episode.lost += records;
                episode.last_loss = now;
            }
            None => {
                tracing::warn!(
                    event = "perf.write_failed",
                    role = self.role,
                    error = cause,
                    "perf records are not being written"
                );
                self.episode = Some(Episode {
                    lost: records,
                    last_loss: now,
                });
            }
        }
    }

    fn end_episode(&mut self) {
        if let Some(episode) = self.episode.take() {
            tracing::warn!(
                event = "perf.write_recovered",
                role = self.role,
                lost_records = episode.lost,
                "perf records are being written again"
            );
        }
    }
}
