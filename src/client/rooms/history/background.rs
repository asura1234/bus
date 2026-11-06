//! One coalescing worker owns expensive layouts. Compose only exchanges Arcs.
use super::super::thumbnails::Thumbnails;
use super::{History, Line, LocalSendStatus};
use crate::messaging::{
    coordinator::BusSnapshot,
    model::{PromptId, RoomId},
};
use crate::protocol::kitty::HostCellSize;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Condvar, Mutex,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    revision: u64,
    room: RoomId,
    width: u16,
    minute: u64,
    cell: Option<(u32, u32)>,
    filters: crate::messaging::model::RoomHistoryFilters,
    limit: usize,
    submissions: u64,
}

impl Key {
    fn same_geometry(self, other: Self) -> bool {
        self.room == other.room
            && self.width == other.width
            && self.cell == other.cell
            && self.filters == other.filters
            && self.submissions == other.submissions
    }
}

struct Job {
    key: Key,
    generation: u64,
    snapshot: Arc<BusSnapshot>,
    submissions: BTreeMap<PromptId, LocalSendStatus>,
    now: u64,
}

struct Layout {
    key: Key,
    lines: Arc<[Line]>,
    filters: crate::messaging::model::RoomHistoryFilters,
    age_valid: Range<u64>,
    has_older: bool,
}

impl Layout {
    fn reusable(&self, key: Key, now: u64) -> bool {
        self.key.revision == key.revision
            && self.key.room == key.room
            && self.key.width == key.width
            && self.key.cell == key.cell
            && self.key.filters == key.filters
            && self.key.limit == key.limit
            && self.key.submissions == key.submissions
            && (self.key.minute == key.minute || self.age_valid.contains(&now))
    }
}

#[derive(Default)]
struct Mailbox {
    next: Option<Job>,
    result: Option<Layout>,
    stopped: bool,
}

#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    generation: AtomicU64,
    ready: AtomicBool,
    #[cfg(test)]
    progress: TestCounters,
}

#[cfg(test)]
#[derive(Default)]
struct TestCounters {
    exchanges: AtomicU64,
    cancelled: AtomicU64,
    started: AtomicU64,
    finished: AtomicU64,
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct TestProgress {
    pub(super) generation: u64,
    pub(super) exchanges: u64,
    pub(super) cancelled: u64,
    pub(super) started: u64,
    pub(super) finished: u64,
    pub(super) worker_stopped: bool,
}

pub(super) struct Cancellation {
    shared: Arc<Shared>,
    generation: u64,
}

impl Cancellation {
    pub(super) fn cancelled(&self) -> bool {
        let cancelled = self.shared.generation.load(Ordering::Relaxed) != self.generation;
        #[cfg(test)]
        {
            self.shared
                .progress
                .exchanges
                .fetch_add(1, Ordering::Relaxed);
            if cancelled {
                self.shared
                    .progress
                    .cancelled
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        cancelled
    }
}

pub(super) struct Background {
    shared: Arc<Shared>,
    requested: Option<Key>,
    displayed_room: Option<RoomId>,
    layouts: BTreeMap<RoomId, Layout>,
    _thread: std::thread::JoinHandle<()>,
}

impl Background {
    #[cfg(test)]
    pub(super) fn test_check_stale_generation(&self) {
        let generation = self.shared.generation.fetch_add(1, Ordering::Relaxed);
        assert!(Cancellation {
            shared: Arc::clone(&self.shared),
            generation,
        }
        .cancelled());
    }

    #[cfg(test)]
    pub(super) fn test_progress(&self) -> TestProgress {
        TestProgress {
            generation: self.shared.generation.load(Ordering::Relaxed),
            exchanges: self.shared.progress.exchanges.load(Ordering::Relaxed),
            cancelled: self.shared.progress.cancelled.load(Ordering::Relaxed),
            started: self.shared.progress.started.load(Ordering::Relaxed),
            finished: self.shared.progress.finished.load(Ordering::Relaxed),
            worker_stopped: self._thread.is_finished(),
        }
    }

    #[cfg(test)]
    pub(super) fn is_current(
        &self,
        revision: u64,
        room: RoomId,
        width: u16,
        now: u64,
        cell: Option<(u32, u32)>,
    ) -> bool {
        let key = Key {
            revision,
            room,
            width,
            minute: now / 60_000,
            cell,
            filters: self
                .requested
                .map_or_else(Default::default, |key| key.filters),
            limit: self
                .requested
                .map_or(super::window::PAGE_RECORDS, |key| key.limit),
            submissions: self.requested.map_or(0, |key| key.submissions),
        };
        self.layouts
            .get(&room)
            .is_some_and(|layout| layout.reusable(key, now))
    }

    fn take_completed(&mut self, key: Key) -> Option<Layout> {
        let mut mailbox = self
            .shared
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.shared.ready.store(false, Ordering::Relaxed);
        mailbox
            .result
            .take()
            .filter(|layout| layout.key.same_geometry(key))
    }

    fn enqueue(
        &mut self,
        key: Key,
        snapshot: Arc<BusSnapshot>,
        now: u64,
        submissions: BTreeMap<PromptId, LocalSendStatus>,
    ) {
        if self.requested == Some(key) {
            return;
        }
        // Poll/draft revisions must not starve a cold layout by cancelling it
        // before it can finish. Only room/geometry changes cancel running work.
        let generation = if self
            .requested
            .is_some_and(|previous| previous.same_geometry(key))
        {
            self.shared.generation.load(Ordering::Relaxed)
        } else {
            self.shared.generation.fetch_add(1, Ordering::Relaxed) + 1
        };
        self.requested = Some(key);
        let job = Job {
            key,
            generation,
            snapshot,
            submissions,
            now,
        };
        let mut mailbox = self
            .shared
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // At most one running job and one pending job, even during resize storms.
        mailbox.next = Some(job);
        self.shared.wake.notify_one();
    }

    fn start() -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("bus-history".into())
            .spawn(move || run(worker))
            .map_err(|error| format!("start history worker: {error}"))?;
        Ok(Self {
            shared,
            requested: None,
            displayed_room: None,
            layouts: BTreeMap::new(),
            _thread: thread,
        })
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        // Do not join a renderer from the UI thread. It checks cancellation at
        // every exchange, then releases its caches and exits on its own thread.
        let mut mailbox = self
            .shared
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mailbox.stopped = true;
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
        self.shared.wake.notify_one();
    }
}

impl History {
    pub(in crate::client::rooms) fn enable_background(&mut self) -> Result<(), String> {
        if self.background.is_none() {
            self.background = Some(Background::start()?);
        }
        Ok(())
    }

    /// A completed layout requests one new frame, including when every agent
    /// is idle. Applying it waits until compose has captured the old scroll anchor.
    pub(in crate::client::rooms) fn poll(&self) -> bool {
        self.background
            .as_ref()
            .is_some_and(|background| background.shared.ready.swap(false, Ordering::Relaxed))
    }

    fn reset_metrics(&mut self) {
        self.rebuilt = 0;
        self.rendered = 0;
        self.projected = 0;
    }

    #[cfg(test)]
    pub(in crate::client::rooms) fn request_lines(
        &mut self,
        snapshot: Arc<BusSnapshot>,
        room: RoomId,
        width: u16,
        now: u64,
        thumbnails: &mut Thumbnails,
    ) {
        let filters = snapshot
            .state
            .room(room)
            .map_or_else(Default::default, |room| room.history_filters);
        self.request_lines_filtered(snapshot, room, width, now, thumbnails, filters);
    }

    #[cfg(test)]
    pub(in crate::client::rooms) fn request_lines_filtered(
        &mut self,
        snapshot: Arc<BusSnapshot>,
        room: RoomId,
        width: u16,
        now: u64,
        thumbnails: &mut Thumbnails,
        filters: crate::messaging::model::RoomHistoryFilters,
    ) {
        self.request_lines_with_submissions(
            snapshot,
            room,
            width,
            now,
            thumbnails,
            filters,
            Default::default(),
        );
    }

    pub(in crate::client::rooms) fn request_lines_with_submissions(
        &mut self,
        snapshot: Arc<BusSnapshot>,
        room: RoomId,
        width: u16,
        now: u64,
        thumbnails: &mut Thumbnails,
        filters: crate::messaging::model::RoomHistoryFilters,
        submissions: BTreeMap<PromptId, LocalSendStatus>,
    ) {
        static REQUEST: crate::utils::perf::Timer =
            crate::utils::perf::Timer::new("client.history.request");
        let _timer = REQUEST.start();
        self.reset_metrics();
        let switching = self.window_room != Some((room, filters));
        self.select_window(room, filters);
        if switching {
            if let Some(layout) = self
                .background
                .as_ref()
                .and_then(|background| background.layouts.get(&room))
                .filter(|layout| layout.filters == filters)
            {
                self.record_limit = layout.key.limit;
            }
        }
        let Some(background) = self.background.as_mut() else {
            if let Some(room) = snapshot.state.room(room) {
                self.lines_with_submissions(
                    &snapshot.state,
                    room,
                    width,
                    snapshot.revision,
                    now,
                    thumbnails,
                    filters,
                    &submissions,
                );
            }
            return;
        };
        let mut key = Key {
            revision: snapshot.revision,
            room,
            width,
            minute: now / 60_000,
            cell: thumbnails.layout_key(),
            filters,
            limit: self.record_limit,
            submissions: super::submission_signature(&submissions),
        };
        background
            .layouts
            .retain(|id, _| snapshot.state.room(*id).is_some());
        if background.displayed_room != Some(room) {
            self.lines = background
                .layouts
                .get(&room)
                .map_or_else(Arc::default, |layout| Arc::clone(&layout.lines));
            self.displayed_filters = background
                .layouts
                .get(&room)
                .map_or_else(Default::default, |layout| layout.filters);
            background.displayed_room = Some(room);
        }
        if let Some(layout) = background.take_completed(key) {
            self.lines = Arc::clone(&layout.lines);
            self.displayed_filters = layout.filters;
            self.loaded_limit = layout.key.limit;
            self.has_older = layout.has_older;
            background.layouts.insert(layout.key.room, layout);
            // Empty/sparse recent batches must continue without a scroll: the
            // newest matching record may be beyond a batch of peer traffic.
            if self.has_older && self.loaded_limit == self.record_limit && self.lines.len() < 80 {
                self.record_limit = self
                    .record_limit
                    .saturating_add(super::window::PAGE_RECORDS);
                key.limit = self.record_limit;
            }
        }
        if let Some(layout) = background
            .layouts
            .get(&room)
            .filter(|layout| layout.reusable(key, now))
        {
            self.lines = Arc::clone(&layout.lines);
            self.displayed_filters = layout.filters;
            self.loaded_limit = layout.key.limit;
            self.has_older = layout.has_older;
            if background.requested != Some(key) {
                background.shared.generation.fetch_add(1, Ordering::Relaxed);
                let mut mailbox = background
                    .shared
                    .mailbox
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                mailbox.next = None;
            }
            background.requested = Some(key);
            return;
        }
        if let Some(layout) = background.layouts.get(&room) {
            self.lines = if layout.filters == filters && layout.key.submissions == key.submissions {
                Arc::clone(&layout.lines)
            } else {
                Arc::default()
            };
            self.displayed_filters = layout.filters;
        }
        background.enqueue(key, snapshot, now, submissions);
    }
}

fn run(shared: Arc<Shared>) {
    let mut rooms: BTreeMap<RoomId, History> = BTreeMap::new();
    let mut thumbnails = Thumbnails::default();
    loop {
        let job = {
            let mut mailbox = shared.mailbox.lock().unwrap_or_else(|e| e.into_inner());
            while mailbox.next.is_none() && !mailbox.stopped {
                mailbox = shared.wake.wait(mailbox).unwrap_or_else(|e| e.into_inner());
            }
            if mailbox.stopped {
                return;
            }
            mailbox.next.take().expect("pending history job after wait")
        };
        #[cfg(test)]
        shared.progress.started.fetch_add(1, Ordering::Relaxed);
        rooms.retain(|id, _| job.snapshot.state.room(*id).is_some());
        let Some(room) = job.snapshot.state.room(job.key.room) else {
            continue;
        };
        let history = rooms.entry(room.id).or_default();
        history.cancellation = Some(Cancellation {
            shared: Arc::clone(&shared),
            generation: job.generation,
        });
        thumbnails.set_cell(job.key.cell.map(|(width_px, height_px)| HostCellSize {
            width_px,
            height_px,
        }));
        history.select_window(room.id, job.key.filters);
        history.record_limit = job.key.limit;
        history.lines_with_submissions(
            &job.snapshot.state,
            room,
            job.key.width,
            job.key.revision,
            job.now,
            &mut thumbnails,
            job.key.filters,
            &job.submissions,
        );
        #[cfg(test)]
        shared.progress.finished.fetch_add(1, Ordering::Relaxed);
        let layout = Layout {
            key: job.key,
            lines: Arc::clone(&history.lines),
            filters: job.key.filters,
            age_valid: history.age_valid.clone(),
            has_older: history.has_older,
        };
        let mut mailbox = shared.mailbox.lock().unwrap_or_else(|e| e.into_inner());
        if !mailbox.stopped && shared.generation.load(Ordering::Relaxed) == job.generation {
            mailbox.result = Some(layout);
            shared.ready.store(true, Ordering::Relaxed);
        }
    }
}
