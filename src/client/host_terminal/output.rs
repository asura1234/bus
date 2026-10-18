//! One host-output owner. Producers retain semantic frames, never encoded diffs.
mod metrics;
mod queue;
mod routing;
#[cfg(test)]
#[path = "tests/output_test.rs"]
mod tests;

use crate::protocol::{
    ansi::{frame_with_drawn_cursor, BlitEncoder},
    wire::{CursorState, FrameData, PaneSurfacePatchRow},
};
use metrics::OutputMetrics;
use queue::{Command, Frame, Handle, Patch, Shared};
pub(in crate::client) use routing::{busy, stdout, suspend};
use std::{
    io::{self, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    thread::JoinHandle,
};

pub(in crate::client) struct TerminalOutput {
    handle: Handle,
    worker: Option<JoinHandle<()>>,
    latest: Option<FrameData>,
    sequence: u64,
    invalidate: bool,
}

impl TerminalOutput {
    pub(in crate::client) fn start() -> io::Result<Self> {
        Self::with_writer(io::stdout())
    }

    fn with_writer(writer: impl Write + Send + 'static) -> io::Result<Self> {
        Self::with_metrics(writer, &metrics::OUTPUT)
    }

    fn with_metrics(
        writer: impl Write + Send + 'static,
        metrics: &'static OutputMetrics,
    ) -> io::Result<Self> {
        let handle = Handle(Arc::new(Shared::new(metrics)));
        let owner = handle.clone();
        let worker = std::thread::Builder::new()
            .name("host-output".into())
            .spawn(move || run(writer, &owner))?;
        Ok(Self {
            handle,
            worker: Some(worker),
            latest: None,
            sequence: 0,
            invalidate: false,
        })
    }

    pub(in crate::client) fn route(&self) -> routing::Route {
        routing::Route::new(self.handle.clone())
    }

    pub(in crate::client) fn invalidate(&mut self) {
        self.invalidate = true;
        self.latest = None;
    }

    pub(in crate::client) fn frame(
        &mut self,
        frame: FrameData,
        repaint: bool,
        drawn_cursor: bool,
    ) -> io::Result<()> {
        self.submit(frame, repaint, drawn_cursor, None)
    }

    fn submit(
        &mut self,
        frame: FrameData,
        repaint: bool,
        drawn_cursor: bool,
        patch: Option<Patch>,
    ) -> io::Result<()> {
        self.handle.check_frame(&frame)?;
        let mut latest = frame.clone();
        latest.graphics.clear();
        self.sequence = self.sequence.wrapping_add(1);
        self.handle.push(Command::Frame(Frame {
            data: frame,
            repaint,
            invalidate: self.invalidate,
            drawn_cursor,
            sequence: self.sequence,
            patch,
        }))?;
        self.latest = Some(latest);
        self.invalidate = false;
        Ok(())
    }

    pub(in crate::client) fn patch(
        &mut self,
        rows: &[PaneSurfacePatchRow],
        cursor: Option<CursorState>,
        drawn_cursor: bool,
    ) -> io::Result<bool> {
        let Some(mut frame) = self.latest.clone() else {
            return Ok(false);
        };
        for row in rows {
            if row.y >= frame.height
                || usize::from(row.x) + row.cells.len() > usize::from(frame.width)
            {
                return Ok(false);
            }
            let start = usize::from(row.y) * usize::from(frame.width) + usize::from(row.x);
            let Some(target) = frame.cells.get_mut(start..start + row.cells.len()) else {
                return Ok(false);
            };
            target.clone_from_slice(&row.cells);
        }
        frame.cursor = cursor.clone();
        self.submit(
            frame,
            false,
            drawn_cursor,
            Some(Patch {
                base: self.sequence,
                rows: rows.to_vec(),
                cursor,
            }),
        )?;
        Ok(true)
    }

    pub(in crate::client) fn graphics(&self, bytes: &[u8]) -> io::Result<()> {
        self.handle.push(Command::Graphics(bytes.to_vec()))
    }

    /// An encoded legacy frame is a lossless barrier, including its cursor effects.
    pub(in crate::client) fn terminal_bytes(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        self.invalidate();
        self.handle.push(Command::Bytes(bytes))
    }

    pub(in crate::client) fn feedback(&self) -> io::Result<Option<bool>> {
        self.handle.feedback()
    }

    pub(in crate::client) fn finish(&mut self) -> io::Result<()> {
        self.handle.close();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                return Err(io::Error::other("host output owner panicked"));
            }
        }
        self.handle.check()
    }
}

impl Drop for TerminalOutput {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn run(mut writer: impl Write, handle: &Handle) {
    let mut encoder = BlitEncoder::new();
    let mut sequence = 0;
    while let Some(command) = handle.next() {
        let result = catch_unwind(AssertUnwindSafe(|| {
            write_command(
                &mut writer,
                &mut encoder,
                &mut sequence,
                &command.command,
                handle.0.metrics,
            )
        }))
        .unwrap_or_else(|_| Err(io::Error::other("host output owner panicked")));
        handle.complete(command, result);
    }
}

fn write_command(
    writer: &mut impl Write,
    encoder: &mut BlitEncoder,
    sequence: &mut u64,
    command: &Command,
    metrics: &'static OutputMetrics,
) -> io::Result<Option<bool>> {
    match command {
        Command::Frame(request) => {
            if !request.repaint && !request.invalidate {
                if let Some(patch) = request
                    .patch
                    .as_ref()
                    .filter(|patch| patch.base == *sequence)
                {
                    if write_patch(writer, encoder, patch, request.drawn_cursor, metrics)? {
                        *sequence = request.sequence;
                        return Ok(Some(false));
                    }
                }
            }
            let encode = metrics.frame.encode.start();
            if request.invalidate {
                encoder.invalidate();
            }
            let frame = if request.drawn_cursor {
                frame_with_drawn_cursor(request.data.clone())
            } else {
                request.data.clone()
            };
            let cleared = encoder.clears_before(&frame);
            let encoded = if request.drawn_cursor {
                encoder.encode_with_suppressed_visible_cursor(&frame, request.repaint)
            } else {
                encoder.encode(&frame, request.repaint)
            };
            drop(encode);
            metrics.frame.encoded_bytes.add(
                encoded.bytes.len() as u64
                    + frame.graphics.len() as u64
                    + if frame.graphics.is_empty() { 0 } else { 4 },
            );
            {
                let _write = metrics.frame.write.start();
                super::frame_output::write_encoded_frame_with_graphics(
                    &mut *writer,
                    &encoded.bytes,
                    &frame.graphics,
                )?;
            }
            {
                let _flush = metrics.frame.flush.start();
                writer.flush()?;
            }
            // The only baseline commit: all bytes and the flush succeeded.
            encoder.commit(frame, encoded);
            *sequence = request.sequence;
            Ok(Some(cleared))
        }
        Command::Bytes(bytes) => {
            super::frame_output::record_received_kitty_graphics(bytes);
            writer.write_all(bytes)?;
            writer.flush()?;
            Ok(None)
        }
        Command::Graphics(bytes) => {
            super::frame_output::write_encoded_frame_with_graphics(&mut *writer, &[], bytes)?;
            writer.flush()?;
            Ok(None)
        }
    }
}

fn write_patch(
    writer: &mut impl Write,
    encoder: &mut BlitEncoder,
    patch: &Patch,
    drawn_cursor: bool,
    metrics: &'static OutputMetrics,
) -> io::Result<bool> {
    let encode = metrics.patch.encode.start();
    let rows = if drawn_cursor {
        let Some(rows) = encoder.patch_rows_with_drawn_cursor(&patch.rows, patch.cursor.as_ref())
        else {
            return Ok(false);
        };
        rows
    } else {
        patch.rows.clone()
    };
    let Some(encoded) = encoder.encode_patch(&rows, patch.cursor.clone(), drawn_cursor) else {
        return Ok(false);
    };
    drop(encode);
    metrics.patch.encoded_bytes.add(encoded.bytes.len() as u64);
    {
        let _write = metrics.patch.write.start();
        writer.write_all(&encoded.bytes)?;
    }
    {
        let _flush = metrics.patch.flush.start();
        writer.flush()?;
    }
    if !encoder.commit_patch(&rows, patch.cursor.clone(), encoded) {
        return Err(io::Error::other("host output patch baseline lost"));
    }
    Ok(true)
}
