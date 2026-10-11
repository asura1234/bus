//! Host waits: each polls the screen (or the effect log) until its condition
//! holds or its timeout passes, and explains a timeout with what it saw.
use super::actions::{excerpt, needle};
use super::{failure, screen, Command, Grid, Host, Target};
use crate::devtools::tui::effects;
use crate::devtools::tui::protocol::{Region, Style};
use crate::devtools::tui::screen::Cell;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(30);

impl Host {
    pub(super) fn wait(&self, command: Command) -> Value {
        match command {
            Command::WaitText {
                text,
                regex,
                gone,
                rows,
                timeout_ms,
            } => self.wait_text(&text, regex, gone, rows, Duration::from_millis(timeout_ms)),
            Command::WaitStable {
                stable_ms,
                region,
                timeout_ms,
            } => self.wait_stable(
                Duration::from_millis(stable_ms),
                region,
                Duration::from_millis(timeout_ms),
            ),
            Command::WaitChanged { region, timeout_ms } => {
                self.wait_changed(region, Duration::from_millis(timeout_ms))
            }
            Command::WaitStyle {
                target,
                style,
                timeout_ms,
            } => self.wait_style(&target, &style, Duration::from_millis(timeout_ms)),
            Command::WaitEffect {
                kind,
                target,
                all,
                timeout_ms,
            } => self.wait_effect(
                &kind,
                target.as_deref(),
                all,
                Duration::from_millis(timeout_ms),
            ),
            _ => failure("usage", "not a wait"),
        }
    }

    /// Polls `check` until it returns a response or the timeout passes.
    fn poll(
        &self,
        timeout: Duration,
        mut check: impl FnMut(&Grid) -> Option<Value>,
        timed_out: impl FnOnce(&Grid) -> Value,
    ) -> Value {
        let started = Instant::now();
        loop {
            let grid = self.grid();
            if let Some(mut done) = check(&grid) {
                done["waited_ms"] = json!(started.elapsed().as_millis() as u64);
                return done;
            }
            if let Some(reason) = self.exited() {
                return failure("bus_exited", reason);
            }
            if started.elapsed() >= timeout {
                return timed_out(&grid);
            }
            std::thread::sleep(POLL);
        }
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
        self.poll(
            timeout,
            |grid| {
                let matches = screen::find(grid, &needle, rows);
                (matches.is_empty() == gone).then(|| json!({"ok": true, "matches": matches}))
            },
            |grid| {
                let what = if gone { "still visible" } else { "not visible" };
                let mut response = failure(
                    "timeout",
                    format!("{text:?} {what} after {} ms", timeout.as_millis()),
                );
                response["excerpt"] = json!(excerpt(grid, rows));
                response
            },
        )
    }

    /// Styled cells must stay the same for `stable`; colours count, not only text.
    fn wait_stable(&self, stable: Duration, region: Region, timeout: Duration) -> Value {
        let mut last = region_cells(&self.grid(), region);
        let mut since = Instant::now();
        self.poll(
            timeout,
            |grid| {
                // A dead Bus's last screen never changes; that is not stable.
                if self.exited().is_some() {
                    return None;
                }
                let current = region_cells(grid, region);
                if current != last {
                    last = current;
                    since = Instant::now();
                    None
                } else {
                    (since.elapsed() >= stable).then(|| json!({"ok": true}))
                }
            },
            |grid| {
                let mut response = failure(
                    "timeout",
                    format!(
                        "region never stayed unchanged for {} ms within {} ms",
                        stable.as_millis(),
                        timeout.as_millis()
                    ),
                );
                response["excerpt"] = json!(excerpt(grid, region.rows));
                response
            },
        )
    }

    fn wait_changed(&self, region: Region, timeout: Duration) -> Value {
        let Some(before) = self.before_input.borrow().clone() else {
            return failure("usage", "wait --changed needs an input before it");
        };
        let before = region_cells(&before, region);
        self.poll(
            timeout,
            |grid| (region_cells(grid, region) != before).then(|| json!({"ok": true})),
            |grid| {
                let mut response = failure(
                    "timeout",
                    format!(
                        "region still as it was before the last input after {} ms",
                        timeout.as_millis()
                    ),
                );
                response["excerpt"] = json!(excerpt(grid, region.rows));
                response
            },
        )
    }

    fn wait_style(&self, target: &Target, style: &Style, timeout: Duration) -> Value {
        let mut seen = Value::Null;
        let mut last_error = None;
        let response = self.poll(
            timeout,
            |grid| match self.target_cells(grid, target) {
                Ok(cells) => {
                    let wrong: Vec<&Cell> = cells
                        .iter()
                        .filter(|cell| !cell.symbol.trim().is_empty())
                        .filter(|cell| !style_holds(cell, style))
                        .collect();
                    seen = json!(cells.iter().take(8).collect::<Vec<_>>());
                    last_error = None;
                    (wrong.is_empty() && !cells.is_empty())
                        .then(|| json!({"ok": true, "cells": cells.len()}))
                }
                Err(error) => {
                    last_error = Some(error);
                    None
                }
            },
            |_| Value::Null,
        );
        if !response.is_null() {
            return response;
        }
        if let Some(error) = last_error {
            return error;
        }
        let mut response = failure(
            "timeout",
            format!("style never matched within {} ms", timeout.as_millis()),
        );
        response["cells"] = seen;
        response
    }

    fn wait_effect(&self, kind: &str, target: Option<&str>, all: bool, timeout: Duration) -> Value {
        let log = self.paths.effects_log();
        let from = if all { 0 } else { self.effects_mark.get() };
        let started = Instant::now();
        loop {
            let recorded = effects::read(&log);
            if let Some(found) = recorded
                .iter()
                .skip(from)
                .find(|effect| effects::matches(effect, kind, target))
            {
                return json!({"ok": true, "effect": found,
                              "waited_ms": started.elapsed().as_millis() as u64});
            }
            if started.elapsed() >= timeout {
                let mut response = failure(
                    "timeout",
                    format!(
                        "no {kind} effect{} {} within {} ms",
                        target.map(|t| format!(" for {t:?}")).unwrap_or_default(),
                        if all { "ever" } else { "since the last input" },
                        timeout.as_millis()
                    ),
                );
                response["recent"] = json!(recorded.iter().rev().take(10).collect::<Vec<_>>());
                return response;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The cells a target covers: one cell, a text match, or a hit's region.
    pub(super) fn target_cells(&self, grid: &Grid, target: &Target) -> Result<Vec<Cell>, Value> {
        let (row, col, width, height) = match target {
            Target::Cell { row, col } => (*row, *col, 1, 1),
            Target::Text {
                text,
                regex,
                nth,
                rows,
            } => {
                let needle = needle(text, *regex)?;
                let matches = screen::find(grid, &needle, *rows);
                let found = matches
                    .get(nth.unwrap_or(1).saturating_sub(1))
                    .ok_or_else(|| failure("not_found", format!("no visible text {text:?}")))?;
                (found.row, found.col, found.width, 1)
            }
            Target::Hit { name } => {
                let hit = self.find_hit(name)?;
                (hit.row, hit.col, hit.width, hit.height)
            }
        };
        Ok(region_cells(
            grid,
            Region {
                rows: Some((row, row + height.saturating_sub(1))),
                cols: Some((col, col + width.saturating_sub(1))),
            },
        )
        .into_iter()
        .flatten()
        .collect())
    }
}

pub(super) fn region_cells(grid: &Grid, region: Region) -> Vec<Vec<Cell>> {
    let last_row = grid.rows.saturating_sub(1);
    let last_col = grid.cols.saturating_sub(1);
    let (first_row, end_row) = region.rows.unwrap_or((0, last_row));
    let (first_col, end_col) = region.cols.unwrap_or((0, last_col));
    (first_row..=end_row.min(last_row))
        .map(|row| {
            let cells = &grid.cells[usize::from(row)];
            (first_col..=end_col.min(last_col))
                .filter_map(|col| cells.get(usize::from(col)).cloned())
                .collect()
        })
        .collect()
}

pub(super) fn style_holds(cell: &Cell, style: &Style) -> bool {
    let same = |want: &Option<String>, have: &str| {
        want.as_deref()
            .is_none_or(|want| want.eq_ignore_ascii_case(have))
    };
    same(&style.fg, &cell.fg)
        && same(&style.bg, &cell.bg)
        && style.attrs.iter().all(|a| cell.attrs.contains(&a.as_str()))
        && !style
            .no_attrs
            .iter()
            .any(|a| cell.attrs.contains(&a.as_str()))
}
