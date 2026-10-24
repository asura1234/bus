//! One coalescing worker owns expensive layouts. Compose only exchanges Arcs.
use super::super::thumbnails::Thumbnails;
use super::{History, Line};
use crate::messaging::{coordinator::BusSnapshot, model::RoomId};
use crate::protocol::kitty::HostCellSize;
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Condvar, Mutex,
};

const CACHED_ROOMS: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    revision: u64,
    room: RoomId,
    width: u16,
    minute: u64,
    cell: Option<(u32, u32)>,
}

impl Key {
    fn same_geometry(self, other: Self) -> bool {
        self.room == other.room && self.width == other.width && self.cell == other.cell
    }
}

struct Job {
    key: Key,
    generation: u64,
    snapshot: Arc<BusSnapshot>,
    now: u64,
}

struct Layout {
    key: Key,
    lines: Arc<[Line]>,
    age_valid: Range<u64>,
}

impl Layout {
    fn reusable(&self, key: Key, now: u64) -> bool {
        self.key.revision == key.revision
            && self.key.room == key.room
            && self.key.width == key.width
            && self.key.cell == key.cell
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
}

pub(super) struct Cancellation {
    shared: Arc<Shared>,
    generation: u64,
}

impl Cancellation {
    pub(super) fn cancelled(&self) -> bool {
        self.shared.generation.load(Ordering::Relaxed) != self.generation
    }
}

pub(super) struct Background {
    shared: Arc<Shared>,
    requested: Option<Key>,
    displayed_room: Option<RoomId>,
    layouts: VecDeque<Layout>,
    _thread: std::thread::JoinHandle<()>,
}

impl Background {
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
        };
        self.layouts.iter().any(|layout| layout.reusable(key, now))
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
            layouts: VecDeque::new(),
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

    pub(in crate::client::rooms) fn request_lines(
        &mut self,
        snapshot: Arc<BusSnapshot>,
        room: RoomId,
        width: u16,
        now: u64,
        thumbnails: &mut Thumbnails,
    ) {
        static REQUEST: crate::utils::perf::Timer =
            crate::utils::perf::Timer::new("client.history.request");
        let _timer = REQUEST.start();
        let Some(background) = self.background.as_mut() else {
            if let Some(room) = snapshot.state.room(room) {
                self.lines(
                    &snapshot.state,
                    room,
                    width,
                    snapshot.revision,
                    now,
                    thumbnails,
                );
            }
            return;
        };
        self.rebuilt = 0;
        self.rendered = 0;
        let key = Key {
            revision: snapshot.revision,
            room,
            width,
            minute: now / 60_000,
            cell: thumbnails.layout_key(),
        };
        // Never show another room's rows while this room's first layout loads.
        if background.displayed_room != Some(room) {
            self.lines = background
                .layouts
                .iter()
                .find(|layout| layout.key.room == room)
                .map_or_else(Arc::default, |layout| Arc::clone(&layout.lines));
            background.displayed_room = Some(room);
        }
        let completed = {
            let mut mailbox = background
                .shared
                .mailbox
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            background.shared.ready.store(false, Ordering::Relaxed);
            mailbox.result.take()
        };
        if let Some(layout) = completed.filter(|layout| layout.key.same_geometry(key)) {
            background
                .layouts
                .retain(|old| old.key.room != layout.key.room);
            background.layouts.push_back(layout);
            if background.layouts.len() > CACHED_ROOMS {
                background.layouts.pop_front();
            }
        }
        if let Some(index) = background
            .layouts
            .iter()
            .position(|layout| layout.reusable(key, now))
        {
            let layout = background.layouts.remove(index).unwrap();
            self.lines = Arc::clone(&layout.lines);
            background.layouts.push_back(layout);
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
        // Keep the same room's last complete layout during a resize or update.
        if let Some(layout) = background
            .layouts
            .iter()
            .find(|layout| layout.key.room == room)
        {
            self.lines = Arc::clone(&layout.lines);
        }
        if background.requested == Some(key) {
            return;
        }
        // Poll/draft revisions must not starve a cold layout by cancelling it
        // before it can finish. Only room/geometry changes cancel running work.
        let generation = if background
            .requested
            .is_some_and(|previous| previous.same_geometry(key))
        {
            background.shared.generation.load(Ordering::Relaxed)
        } else {
            background.shared.generation.fetch_add(1, Ordering::Relaxed) + 1
        };
        background.requested = Some(key);
        let job = Job {
            key,
            generation,
            snapshot,
            now,
        };
        let mut mailbox = background
            .shared
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // At most one running job and one pending job, even during resize storms.
        mailbox.next = Some(job);
        background.shared.wake.notify_one();
    }
}

fn run(shared: Arc<Shared>) {
    let mut rooms: VecDeque<(RoomId, History)> = VecDeque::new();
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
            mailbox.next.take().unwrap()
        };
        rooms.retain(|(id, _)| job.snapshot.state.room(*id).is_some());
        let Some(room) = job.snapshot.state.room(job.key.room) else {
            continue;
        };
        let index = rooms.iter().position(|(id, _)| *id == room.id);
        let (_, mut history) = index
            .and_then(|index| rooms.remove(index))
            .unwrap_or_else(|| (room.id, History::default()));
        history.cancellation = Some(Cancellation {
            shared: Arc::clone(&shared),
            generation: job.generation,
        });
        thumbnails.set_cell(job.key.cell.map(|(width_px, height_px)| HostCellSize {
            width_px,
            height_px,
        }));
        history.lines(
            &job.snapshot.state,
            room,
            job.key.width,
            job.key.revision,
            job.now,
            &mut thumbnails,
        );
        let layout = Layout {
            key: job.key,
            lines: Arc::clone(&history.lines),
            age_valid: history.age_valid.clone(),
        };
        rooms.push_back((room.id, history));
        if rooms.len() > CACHED_ROOMS {
            rooms.pop_front();
        }
        let mut mailbox = shared.mailbox.lock().unwrap_or_else(|e| e.into_inner());
        if !mailbox.stopped && shared.generation.load(Ordering::Relaxed) == job.generation {
            mailbox.result = Some(layout);
            shared.ready.store(true, Ordering::Relaxed);
        }
    }
}
