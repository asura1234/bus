//! Host verbs: reading the screen, sending input, waiting.
use super::{
    failure, input, pty_size, screen, Button, Command, Grid, Host, Mods, Needle, Target,
    DOUBLE_CLICK_GAP, FRAME_WAIT, SETTLE_MAX, SETTLE_QUIET,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

impl Host {
    /// Writes input, then waits for the first frame after it and a short quiet
    /// spell. A missing frame is reported, never treated as failure: input can
    /// be delivered and change nothing.
    fn act(&self, chunks: &[Vec<u8>], gap: Duration) -> Value {
        let before = self.grid();
        let (frames_before, _) = self.counters();
        let started = Instant::now();
        for (index, chunk) in chunks.iter().enumerate() {
            if index > 0 && !gap.is_zero() {
                std::thread::sleep(gap);
            }
            if let Err(error) = self.write(chunk) {
                return error;
            }
        }
        let frame = self.settle(frames_before, started);
        let after = self.grid();
        let mut response = json!({"ok": true, "frame": frame});
        let diff = row_diff(&before, &after);
        response["changed_rows"] = json!(diff.iter().map(|d| d["row"].clone()).collect::<Vec<_>>());
        response["diff"] = json!(diff.into_iter().take(12).collect::<Vec<_>>());
        response["cursor"] = json!(after.cursor);
        response
    }

    fn settle(&self, frames_before: u64, started: Instant) -> Value {
        let (lock, ready) = &*self.output;
        let deadline = started + FRAME_WAIT;
        {
            let Ok(mut out) = lock.lock() else {
                return Value::Null;
            };
            while out.frames == frames_before {
                let now = Instant::now();
                if now >= deadline || out.exited.is_some() {
                    return Value::Null;
                }
                out = match ready.wait_timeout(out, deadline - now) {
                    Ok((out, _)) => out,
                    Err(_) => return Value::Null,
                };
            }
        }
        self.quiet(SETTLE_QUIET, SETTLE_MAX);
        let (frames, _) = self.counters();
        json!({"seq": frames, "settled_ms": started.elapsed().as_millis() as u64})
    }

    fn resolve(&self, target: &Target) -> Result<(u16, u16), Value> {
        let grid = self.grid();
        match target {
            Target::Cell { row, col } => {
                if *row >= grid.rows || *col >= grid.cols {
                    return Err(failure(
                        "out_of_bounds",
                        format!(
                            "cell {row} {col} is outside the {}x{} screen",
                            grid.cols, grid.rows
                        ),
                    ));
                }
                Ok((*row, *col))
            }
            Target::Text {
                text,
                regex,
                nth,
                rows,
            } => {
                let needle = needle(text, *regex)?;
                let matches = screen::find(&grid, &needle, *rows);
                let chosen = match nth {
                    Some(n) => matches.get(n.saturating_sub(1)),
                    None if matches.len() == 1 => matches.first(),
                    None if matches.is_empty() => None,
                    None => {
                        return Err(failure(
                            "ambiguous",
                            format!(
                                "{} matches for {text:?} at {}; pass --nth N or narrow with --rows",
                                matches.len(),
                                matches
                                    .iter()
                                    .map(|m| format!("{}:{}", m.row, m.col))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        ))
                    }
                };
                let found = chosen
                    .ok_or_else(|| failure("not_found", format!("no visible text {text:?}")))?;
                Ok((found.row, found.col + found.width.saturating_sub(1) / 2))
            }
        }
    }

    pub(super) fn handle(&mut self, command: Command) -> Value {
        if let Some(reason) = self.exited() {
            if !matches!(command, Command::Stop | Command::Status) {
                return failure("bus_exited", reason);
            }
        }
        match command {
            Command::Status => self.status(),
            Command::Snapshot { rows } => self.snapshot(rows),
            Command::Find { text, regex, rows } => match needle(&text, regex) {
                Ok(needle) => {
                    let matches = screen::find(&self.grid(), &needle, rows);
                    json!({"ok": true, "count": matches.len(), "matches": matches})
                }
                Err(error) => error,
            },
            Command::Cells {
                row,
                col,
                width,
                height,
            } => self.cells(row, col, width, height),
            Command::Type { text } => self.act(&[text.into_bytes()], Duration::ZERO),
            Command::Paste { text } => {
                let bracketed = self
                    .output
                    .0
                    .lock()
                    .map(|out| out.screen.bracketed_paste())
                    .unwrap_or(true);
                self.act(&[input::paste(&text, bracketed)], Duration::ZERO)
            }
            Command::Press { keys } => self.press(&keys),
            Command::Click {
                target,
                button,
                mods,
                double,
            } => self.click(&target, &button, &mods, double),
            Command::Drag {
                from,
                to,
                button,
                mods,
            } => self.drag(&from, &to, &button, &mods),
            Command::Scroll {
                target,
                up,
                notches,
                mods,
            } => self.scroll(&target, up, notches, &mods),
            Command::Resize { cols, rows } => self.resize(cols, rows),
            Command::WaitText {
                text,
                regex,
                gone,
                rows,
                timeout_ms,
            } => self.wait_text(&text, regex, gone, rows, Duration::from_millis(timeout_ms)),
            Command::WaitStable {
                stable_ms,
                rows,
                timeout_ms,
            } => self.wait_stable(
                Duration::from_millis(stable_ms),
                rows,
                Duration::from_millis(timeout_ms),
            ),
            Command::Clipboard => {
                let copied = self
                    .output
                    .0
                    .lock()
                    .map(|out| out.clipboard.clone())
                    .unwrap_or_default();
                json!({"ok": true, "copies": copied})
            }
            Command::Stop => json!({"ok": true}),
        }
    }

    fn status(&self) -> Value {
        let (frames, _) = self.counters();
        json!({"ok": true, "session": self.info.name, "host_pid": self.info.host_pid,
               "bus_pid": self.info.bus_pid, "alive": self.exited().is_none(),
               "exited": self.exited(), "size": self.info.size, "seq": frames,
               "data_dir": self.info.data_dir, "run_dir": self.info.run_dir,
               "binary": self.info.binary})
    }

    fn snapshot(&self, rows: Option<(u16, u16)>) -> Value {
        let grid = self.grid();
        let (frames, _) = self.counters();
        json!({"ok": true, "seq": frames, "size": [grid.cols, grid.rows],
               "cursor": grid.cursor, "rows": excerpt(&grid, rows)})
    }

    fn cells(&self, row: u16, col: u16, width: u16, height: u16) -> Value {
        let grid = self.grid();
        let cells: Vec<Value> = (row..row.saturating_add(height).min(grid.rows))
            .flat_map(|y| {
                let grid = &grid;
                (col..col.saturating_add(width).min(grid.cols)).map(move |x| {
                    let mut cell = json!(grid.cells[usize::from(y)][usize::from(x)]);
                    cell["row"] = json!(y);
                    cell["col"] = json!(x);
                    cell
                })
            })
            .collect();
        json!({"ok": true, "cells": cells})
    }

    fn press(&self, keys: &[String]) -> Value {
        let protocol = self
            .output
            .0
            .lock()
            .map(|out| out.screen.keyboard_protocol())
            .unwrap_or(crate::protocol::keys::KeyboardProtocol::Legacy);
        let mut chunks = Vec::new();
        for key in keys {
            match input::encode_key(key, protocol) {
                Ok(bytes) => chunks.push(bytes),
                Err(message) => return failure("usage", message),
            }
        }
        // Separate presses so Bus's input framer sees distinct keys.
        self.act(&chunks, Duration::from_millis(15))
    }

    fn click(&self, target: &Target, button: &str, mods: &str, double: bool) -> Value {
        let (button, mods) = match (Button::parse(button), Mods::parse(mods)) {
            (Ok(b), Ok(m)) => (b, m),
            (Err(e), _) | (_, Err(e)) => return failure("usage", e),
        };
        let (row, col) = match self.resolve(target) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let click = input::click(row, col, button, mods);
        let chunks = if double {
            vec![click.clone(), click]
        } else {
            vec![click]
        };
        let mut response = self.act(&chunks, DOUBLE_CLICK_GAP);
        response["at"] = json!([row, col]);
        response
    }

    fn drag(&self, from: &Target, to: &Target, button: &str, mods: &str) -> Value {
        let (button, mods) = match (Button::parse(button), Mods::parse(mods)) {
            (Ok(b), Ok(m)) => (b, m),
            (Err(e), _) | (_, Err(e)) => return failure("usage", e),
        };
        let (from, to) = match (self.resolve(from), self.resolve(to)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => return e,
        };
        let copied_before = self.clipboard_len();
        let mut response = self.act(
            &input::drag(from, to, button, mods),
            Duration::from_millis(15),
        );
        response["from"] = json!([from.0, from.1]);
        response["to"] = json!([to.0, to.1]);
        let copied = self.clipboard_since(copied_before);
        if !copied.is_empty() {
            response["copied"] = json!(copied);
        }
        response
    }

    fn scroll(&self, target: &Target, up: bool, notches: u16, mods: &str) -> Value {
        let mods = match Mods::parse(mods) {
            Ok(m) => m,
            Err(e) => return failure("usage", e),
        };
        let (row, col) = match self.resolve(target) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let mut response = self.act(
            &[input::scroll(row, col, up, notches, mods)],
            Duration::ZERO,
        );
        response["at"] = json!([row, col]);
        response
    }

    fn clipboard_len(&self) -> usize {
        self.output
            .0
            .lock()
            .map(|out| out.clipboard.len())
            .unwrap_or(0)
    }

    fn clipboard_since(&self, start: usize) -> Vec<String> {
        // A copy is written right after the release; give it a moment.
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            let copies = self
                .output
                .0
                .lock()
                .map(|out| out.clipboard[start.min(out.clipboard.len())..].to_vec())
                .unwrap_or_default();
            if !copies.is_empty() || Instant::now() >= deadline {
                return copies;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Value {
        let before = self.grid();
        let (frames_before, _) = self.counters();
        let started = Instant::now();
        if let Ok(mut out) = self.output.0.lock() {
            out.screen.resize(cols, rows);
        }
        if let Err(error) = self.master.resize(pty_size(cols, rows)) {
            return failure("resize_failed", error.to_string());
        }
        self.info.size = (cols, rows);
        let _ = self.paths.write_info(&self.info);
        if let Ok(mut trace) = self.trace.lock() {
            trace.resize(cols, rows);
        }
        // Bus polls its size every 100 ms, so allow a little longer for the frame.
        let frame = {
            let first = self.settle(frames_before, started);
            if first.is_null() {
                self.settle(frames_before, Instant::now())
            } else {
                first
            }
        };
        let after = self.grid();
        json!({"ok": true, "size": [cols, rows], "frame": frame,
               "changed_rows": row_diff(&before, &after).len(), "cursor": after.cursor})
    }

    fn wait_text(
        &self,
        text: &str,
        regex: bool,
        gone: bool,
        rows: Option<(u16, u16)>,
        timeout: Duration,
    ) -> Value {
        let needle = match needle(text, regex) {
            Ok(needle) => needle,
            Err(error) => return error,
        };
        let started = Instant::now();
        let (lock, ready) = &*self.output;
        loop {
            let grid = self.grid();
            let matches = screen::find(&grid, &needle, rows);
            if matches.is_empty() == gone {
                return json!({"ok": true, "waited_ms": started.elapsed().as_millis() as u64,
                              "matches": matches});
            }
            if let Some(reason) = self.exited() {
                return failure("bus_exited", reason);
            }
            if started.elapsed() >= timeout {
                let what = if gone { "still visible" } else { "not visible" };
                let mut response = failure(
                    "timeout",
                    format!("{text:?} {what} after {} ms", timeout.as_millis()),
                );
                response["excerpt"] = json!(excerpt(&grid, rows));
                return response;
            }
            let Ok(out) = lock.lock() else {
                return failure("bus_exited", "screen lock poisoned");
            };
            let _ = ready.wait_timeout(out, Duration::from_millis(50));
        }
    }

    fn wait_stable(&self, stable: Duration, rows: Option<(u16, u16)>, timeout: Duration) -> Value {
        let started = Instant::now();
        let region = |grid: &Grid| -> Vec<String> {
            let (first, last) = rows.unwrap_or((0, grid.rows.saturating_sub(1)));
            (first..=last.min(grid.rows.saturating_sub(1)))
                .map(|row| grid.row_text(usize::from(row)))
                .collect()
        };
        let mut last = region(&self.grid());
        let mut since = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(20));
            let grid = self.grid();
            let current = region(&grid);
            if current != last {
                last = current;
                since = Instant::now();
            } else if since.elapsed() >= stable {
                return json!({"ok": true, "waited_ms": started.elapsed().as_millis() as u64});
            }
            if started.elapsed() >= timeout {
                let mut response = failure(
                    "timeout",
                    format!(
                        "region never stayed unchanged for {} ms within {} ms",
                        stable.as_millis(),
                        timeout.as_millis()
                    ),
                );
                response["excerpt"] = json!(excerpt(&grid, rows));
                return response;
            }
        }
    }
}

fn needle(text: &str, regex: bool) -> Result<Needle<'_>, Value> {
    if regex {
        regex::Regex::new(text)
            .map(Needle::Regex)
            .map_err(|e| failure("usage", format!("bad regex: {e}")))
    } else {
        Ok(Needle::Text(text))
    }
}

fn excerpt(grid: &Grid, rows: Option<(u16, u16)>) -> Vec<String> {
    let (first, last) = rows.unwrap_or((0, grid.rows.saturating_sub(1)));
    (first..=last.min(grid.rows.saturating_sub(1)))
        .map(|row| format!("{row:>3}│ {}", grid.row_text(usize::from(row))))
        .collect()
}

/// Rows whose text changed, with their text before and after.
pub(super) fn row_diff(before: &Grid, after: &Grid) -> Vec<Value> {
    let rows = before.cells.len().max(after.cells.len());
    (0..rows)
        .filter_map(|row| {
            let old = before.row_text(row);
            let new = after.row_text(row);
            (old != new).then(|| json!({"row": row, "before": old, "after": new}))
        })
        .collect()
}
