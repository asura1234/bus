use crate::protocol::wire::{CellData, CursorState, FrameData, PaneSurfacePatchRow};
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Condvar, Mutex, MutexGuard},
};

// Includes the command currently being encoded/written. The semantic baseline
// and latest desired frame are each separately bounded by the same frame cap.
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_COMMANDS: usize = 256;

pub(super) struct Frame {
    pub(super) data: FrameData,
    pub(super) repaint: bool,
    pub(super) invalidate: bool,
    pub(super) drawn_cursor: bool,
    pub(super) sequence: u64,
    pub(super) patch: Option<Patch>,
}

pub(super) struct Patch {
    pub(super) base: u64,
    pub(super) rows: Vec<PaneSurfacePatchRow>,
    pub(super) cursor: Option<CursorState>,
}

pub(super) enum Command {
    Frame(Frame),
    Bytes(Vec<u8>),
    Graphics(Vec<u8>),
}

impl Command {
    fn cost(&self) -> usize {
        match self {
            Self::Frame(frame) => {
                frame_cost(&frame.data).saturating_add(frame.patch.as_ref().map_or(0, |p| {
                    p.rows
                        .iter()
                        .map(|r| {
                            r.cells.len() * std::mem::size_of::<CellData>()
                                + r.cells.iter().map(|c| c.symbol.len()).sum::<usize>()
                        })
                        .sum()
                }))
            }
            Self::Bytes(bytes) | Self::Graphics(bytes) => bytes.len(),
        }
        .saturating_add(std::mem::size_of::<Self>())
    }
}

fn frame_cost(frame: &FrameData) -> usize {
    frame
        .cells
        .len()
        .saturating_mul(std::mem::size_of::<CellData>())
        .saturating_add(
            frame
                .cells
                .iter()
                .map(|cell| cell.symbol.len())
                .sum::<usize>(),
        )
        .saturating_add(
            frame
                .hyperlinks
                .len()
                .saturating_mul(std::mem::size_of::<String>()),
        )
        .saturating_add(frame.hyperlinks.iter().map(String::len).sum::<usize>())
        .saturating_add(frame.graphics.len())
}

pub(super) struct Queued {
    pub(super) command: Command,
    cost: usize,
}

#[derive(Default)]
struct State {
    pending: VecDeque<Queued>,
    bytes: usize,
    active: bool,
    closed: bool,
    error: Option<(io::ErrorKind, String)>,
    write_failed: bool,
    acknowledged: Option<bool>,
    #[cfg(test)]
    presented: u64,
}

#[derive(Default)]
pub(super) struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
#[derive(Clone)]
pub(super) struct Handle(pub(super) Arc<Shared>);

impl Handle {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> (usize, usize, u64) {
        let state = self.lock();
        (state.pending.len(), state.bytes, state.presented)
    }

    pub(super) fn check(&self) -> io::Result<()> {
        check(&self.lock())
    }

    pub(super) fn check_frame(&self, frame: &FrameData) -> io::Result<()> {
        // Also bound worst-case ANSI expansion before the encoder allocates:
        // each cell can change style/cursor/link, and can emit its full URI.
        // Maximal SGR is 58 bytes, CUP 14, and OSC 8 close/open framing 14.
        // Keep headroom without rejecting ordinary large host windows.
        let ansi_bound = frame.cells.iter().fold(256usize, |size, cell| {
            size.saturating_add(96)
                .saturating_add(cell.symbol.len())
                .saturating_add(
                    cell.hyperlink
                        .and_then(|index| frame.hyperlinks.get(index as usize))
                        .map_or(0, |url| url.len().saturating_mul(2)),
                )
        });
        if frame_cost(frame) > MAX_BYTES / 2 || ansi_bound > MAX_BYTES / 2 {
            return self.fail(io::Error::other("host output frame exceeds memory limit"));
        }
        self.check()
    }

    pub(super) fn push(&self, mut command: Command) -> io::Result<()> {
        let cost = command.cost();
        let mut state = self.lock();
        check(&state)?;
        if state.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "host output closed",
            ));
        }
        // A graphics transaction is already encoded by the compositor. Retain
        // it, and never coalesce text across a terminal-effect barrier.
        let replace = matches!((&command, state.pending.back()),
            (Command::Frame(new), Some(Queued { command: Command::Frame(old), .. })) if
            new.data.graphics.is_empty() && old.data.graphics.is_empty()
            && (new.data.width, new.data.height) == (old.data.width, old.data.height));
        let retired = if replace {
            let old = state.pending.pop_back();
            if let (Command::Frame(new), Some(Command::Frame(old))) =
                (&mut command, old.as_ref().map(|old| &old.command))
            {
                new.repaint |= old.repaint;
                new.invalidate |= old.invalidate;
            }
            if let Some(old) = &old {
                state.bytes -= old.cost;
            }
            old
        } else {
            None
        };
        if state.bytes.saturating_add(cost) > MAX_BYTES || state.pending.len() >= MAX_COMMANDS {
            state.error = Some((
                io::ErrorKind::OutOfMemory,
                "host output pending limit exceeded".into(),
            ));
            self.0.wake.notify_all();
            let error = check(&state);
            drop(state);
            drop(retired);
            return error;
        }
        state.bytes += cost;
        state.pending.push_back(Queued { command, cost });
        self.0.wake.notify_all();
        drop(state);
        // Destruction of superseded frame allocations never holds the mailbox.
        drop(retired);
        Ok(())
    }

    pub(super) fn busy(&self) -> bool {
        let state = self.lock();
        state.active || !state.pending.is_empty()
    }

    pub(super) fn feedback(&self) -> io::Result<Option<bool>> {
        let mut state = self.lock();
        check(&state)?;
        Ok(state.acknowledged.take())
    }

    pub(super) fn wait_idle(&self) {
        let mut state = self.lock();
        while state.active || (!state.pending.is_empty() && !state.write_failed) {
            state = self.0.wake.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }

    pub(super) fn close(&self) {
        self.lock().closed = true;
        self.0.wake.notify_all();
    }

    pub(super) fn fail(&self, error: io::Error) -> io::Result<()> {
        self.lock().error = Some((error.kind(), error.to_string()));
        self.0.wake.notify_all();
        Err(error)
    }

    pub(super) fn next(&self) -> Option<Queued> {
        let mut state = self.lock();
        loop {
            if state.write_failed {
                return None;
            }
            if let Some(command) = state.pending.pop_front() {
                state.active = true;
                return Some(command);
            }
            if state.closed || state.error.is_some() {
                return None;
            }
            state = self.0.wake.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }

    pub(super) fn complete(&self, queued: Queued, result: io::Result<Option<bool>>) {
        let mut state = self.lock();
        state.active = false;
        state.bytes -= queued.cost;
        match result {
            Ok(Some(cleared)) => {
                state.acknowledged = Some(state.acknowledged.unwrap_or(false) || cleared);
                #[cfg(test)]
                if let Command::Frame(frame) = &queued.command {
                    state.presented = frame.sequence;
                }
            }
            Ok(None) => {}
            Err(error) => {
                state.write_failed = true;
                state.error = Some((error.kind(), error.to_string()));
            }
        }
        self.0.wake.notify_all();
        drop(state);
        drop(queued);
    }
}

fn check(state: &State) -> io::Result<()> {
    match &state.error {
        Some((kind, message)) => Err(io::Error::new(*kind, message.clone())),
        None => Ok(()),
    }
}
