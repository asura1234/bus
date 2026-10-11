//! Dev variant only: the room UI's clickable regions as of its latest frame,
//! answered by the dev-tier `ui hits` for the TUI driver. The UI and the
//! coordinator run in the same client process, so one shared slot carries them.
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Mutex;

/// One clickable region: a stable name such as `room:review`, `new-agent` or
/// `color-blind`, and its cells.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct UiHit {
    pub name: String,
    pub row: u16,
    pub col: u16,
    pub width: u16,
    pub height: u16,
}

type Frame = (u16, u16, Vec<UiHit>);

static LATEST: Mutex<Option<Frame>> = Mutex::new(None);

/// Called by the room UI after it lays out a frame.
pub(crate) fn publish(cols: u16, rows: u16, hits: Vec<UiHit>) {
    if let Ok(mut latest) = LATEST.lock() {
        let frame = (cols, rows, hits);
        if latest.as_ref() != Some(&frame) {
            *latest = Some(frame);
        }
    }
}

pub(super) fn hits_json() -> Result<Value, String> {
    let latest = LATEST.lock().map_err(|_| "UI hits unavailable")?;
    let (cols, rows, hits) = latest
        .as_ref()
        .ok_or("the room UI has not drawn a frame yet")?;
    Ok(json!({"size": [cols, rows], "hits": hits}))
}
