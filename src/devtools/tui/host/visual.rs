//! Host verbs that look at the session as a whole: screenshots, the room UI's
//! named click targets, the effect log, and the marks each input leaves.
use super::{failure, Grid, Host};
use crate::devtools::tui::{effects, mirror, picture, trace};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// One named region from the room UI's `ui hits`.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Hit {
    pub name: String,
    pub row: u16,
    pub col: u16,
    pub width: u16,
    pub height: u16,
}

impl Hit {
    fn contains(&self, row: u16, col: u16) -> bool {
        (self.row..self.row + self.height).contains(&row)
            && (self.col..self.col + self.width).contains(&col)
    }
}

impl Host {
    /// Remembers the screen and the effect count as they were before an input.
    pub(super) fn mark_input(&self, before: &Grid) {
        *self.before_input.borrow_mut() = Some(before.clone());
        self.effects_mark
            .set(effects::read(&self.paths.effects_log()).len());
    }

    /// The room UI's regions as of its latest frame, through the dev-tier
    /// `ui hits` of this session's own Bus.
    fn hits(&self) -> Result<(Vec<Hit>, (u16, u16)), Value> {
        let output = self.bus(&["ui", "hits"]);
        let reply: Value = serde_json::from_str(output.stdout.trim()).unwrap_or(Value::Null);
        if !output.ok() {
            return Err(failure(
                "hits_unavailable",
                format!(
                    "ui hits failed: {}{}",
                    reply["error"]["message"].as_str().unwrap_or(""),
                    output.stderr.trim()
                ),
            ));
        }
        let result = &reply["result"];
        let hits: Vec<Hit> = serde_json::from_value(result["hits"].clone())
            .map_err(|e| failure("hits_unavailable", format!("unreadable ui hits: {e}")))?;
        let size = serde_json::from_value(result["size"].clone()).unwrap_or((0, 0));
        Ok((hits, size))
    }

    pub(super) fn hits_response(&self) -> Value {
        let (hits, size) = match self.hits() {
            Ok(found) => found,
            Err(error) => return error,
        };
        let grid = self.grid();
        let listed: Vec<Value> = hits
            .iter()
            .map(|hit| {
                json!({"name": hit.name, "row": hit.row, "col": hit.col, "width": hit.width,
                       "height": hit.height, "text": hit_text(&grid, hit)})
            })
            .collect();
        let mut response = json!({"ok": true, "count": listed.len(), "hits": listed});
        if size != (grid.cols, grid.rows) {
            response["stale"] = json!(format!(
                "hits are for a {}x{} frame; the screen is {}x{}",
                size.0, size.1, grid.cols, grid.rows
            ));
        }
        response
    }

    pub(super) fn find_hit(&self, name: &str) -> Result<Hit, Value> {
        let (hits, _) = self.hits()?;
        hits.iter()
            .find(|hit| hit.name == name)
            .cloned()
            .ok_or_else(|| {
                let mut names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
                names.dedup();
                failure(
                    "not_found",
                    format!("no hit named {name:?}; visible: {}", names.join(", ")),
                )
            })
    }

    /// A cell of the named hit that no later hit covers (Bus takes the
    /// topmost), nearest its middle.
    pub(super) fn hit_cell(&self, name: &str) -> Result<(u16, u16), Value> {
        let (hits, _) = self.hits()?;
        let index = hits
            .iter()
            .position(|hit| hit.name == name)
            .ok_or_else(|| self.find_hit(name).err().unwrap_or(Value::Null))?;
        let hit = &hits[index];
        let later = &hits[index + 1..];
        let middle = (hit.row + hit.height / 2, hit.col + hit.width / 2);
        let mut cells: Vec<(u16, u16)> = (hit.row..hit.row + hit.height)
            .flat_map(|row| (hit.col..hit.col + hit.width).map(move |col| (row, col)))
            .collect();
        cells.sort_by_key(|(row, col)| row.abs_diff(middle.0) * 4 + col.abs_diff(middle.1));
        cells
            .into_iter()
            .find(|(row, col)| !later.iter().any(|other| other.contains(*row, *col)))
            .ok_or_else(|| failure("not_found", format!("hit {name:?} is covered by another")))
    }

    pub(super) fn screenshot(&self, real: bool, out: Option<PathBuf>) -> Value {
        let shot = self.shots.get() + 1;
        self.shots.set(shot);
        let suffix = if real { "-real" } else { "" };
        let out = out.unwrap_or_else(|| {
            self.info
                .run_dir
                .join("shots")
                .join(format!("shot-{shot:03}{suffix}.png"))
        });
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !real {
            return match picture::write_png(&self.grid(), &out) {
                Ok((width, height)) => json!({"ok": true, "path": out, "pixels": [width, height],
                    "grid": [self.info.size.0, self.info.size.1],
                    "note": "cells only: inline images are not drawn; use --real to see them"}),
                Err(error) => failure("screenshot_failed", error),
            };
        }
        // Let the frame in flight land in the recording before replaying it.
        self.quiet(Duration::from_millis(200), Duration::from_secs(2));
        if let Ok(mut trace) = self.trace.lock() {
            trace.flush();
        }
        let capture = mirror::Mirror {
            binary: &self.info.binary,
            raw: &self.info.run_dir.join(trace::RAW_OUTPUT),
            work_dir: &self.paths.dir,
            out: &out,
            cols: self.info.size.0,
            rows: self.info.size.1,
        }
        .capture();
        match capture {
            Ok(mut report) => {
                report["ok"] = json!(true);
                report
            }
            Err(error) => failure("screenshot_failed", error),
        }
    }

    pub(super) fn effects(&self, kind: Option<&str>) -> Value {
        let recorded: Vec<Value> = effects::read(&self.paths.effects_log())
            .into_iter()
            .filter(|effect| kind.is_none_or(|kind| effect["kind"].as_str() == Some(kind)))
            .collect();
        json!({"ok": true, "since_last_input": self.effects_mark.get(), "effects": recorded})
    }
}

/// The text a hit covers, row by row.
fn hit_text(grid: &Grid, hit: &Hit) -> String {
    (hit.row..hit.row + hit.height)
        .filter_map(|row| grid.cells.get(usize::from(row)))
        .map(|cells| {
            cells
                .iter()
                .skip(usize::from(hit.col))
                .take(usize::from(hit.width))
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
}
