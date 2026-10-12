//! The `bus-perf-writer` thread: appends perf records off the watchdog.
//!
//! The role file is shared by every process of a role and locked per append, so
//! a write can wait on another process. The watchdog only queues records; when
//! the queue is full or the writer is gone a record is counted as lost, and the
//! writer reports the loss once it writes again.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use crate::utils::logging::RotatingFileState;

/// About four minutes of windows; stall records are rarer.
pub(super) const QUEUE_RECORDS: usize = 256;

enum Message {
    Line(Vec<u8>),
    #[cfg(test)]
    Flush(mpsc::Sender<()>),
}

/// The watchdog's end: queuing never blocks.
pub(super) struct WriterHandle {
    role: &'static str,
    queue: mpsc::SyncSender<Message>,
    /// Records the queue refused since the writer last wrote one.
    dropped: Arc<AtomicU64>,
}

impl WriterHandle {
    /// Starts the writer thread for `out`.
    pub(super) fn spawn(
        role: &'static str,
        out: RotatingFileState,
        capacity: usize,
    ) -> std::io::Result<Self> {
        let (queue, received) = mpsc::sync_channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut writer = PerfWriter::new(role, out, Arc::clone(&dropped));
        std::thread::Builder::new()
            .name("bus-perf-writer".into())
            .spawn(move || {
                while let Ok(message) = received.recv() {
                    match message {
                        Message::Line(line) => writer.write(&line),
                        #[cfg(test)]
                        Message::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })?;
        Ok(Self {
            role,
            queue,
            dropped,
        })
    }

    /// Queues one record; a full queue or a stopped writer loses it, warned
    /// once until the writer writes again.
    pub(super) fn send(&self, line: Vec<u8>) {
        if self.queue.try_send(Message::Line(line)).is_ok() {
            return;
        }
        if self.dropped.fetch_add(1, Ordering::AcqRel) == 0 {
            tracing::warn!(
                event = "perf.write_failed",
                role = self.role,
                error = "writer queue full or stopped",
                "perf records are not being written"
            );
        }
    }

    /// Waits until every record queued so far is written.
    #[cfg(test)]
    pub(super) fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.queue.send(Message::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }
}

/// The writer thread's state: the file and the loss accounting.
pub(super) struct PerfWriter {
    role: &'static str,
    out: RotatingFileState,
    dropped: Arc<AtomicU64>,
    /// Records that failed to append since the last one that succeeded.
    failed: u64,
}

impl PerfWriter {
    pub(super) fn new(role: &'static str, out: RotatingFileState, dropped: Arc<AtomicU64>) -> Self {
        Self {
            role,
            out,
            dropped,
            failed: 0,
        }
    }

    /// Appends one record. A failure is logged once, and the recovery with the
    /// number of records lost, so missing evidence is never mistaken for quiet.
    pub(super) fn write(&mut self, line: &[u8]) {
        match self.out.append(line) {
            Ok(()) => {
                let lost = self.failed + self.dropped.swap(0, Ordering::AcqRel);
                self.failed = 0;
                if lost > 0 {
                    tracing::warn!(
                        event = "perf.write_recovered",
                        role = self.role,
                        lost_records = lost,
                        "perf records are being written again"
                    );
                }
            }
            Err(error) => {
                if self.failed == 0 {
                    tracing::warn!(
                        event = "perf.write_failed",
                        role = self.role,
                        %error,
                        "perf records are not being written"
                    );
                }
                self.failed += 1;
            }
        }
    }
}
