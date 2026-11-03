//! Dev variant only: the room UI's clickable regions and acknowledgement hold,
//! shared with the coordinator because both run in the same client process.
use super::BusEvent;
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
static ACK_HOLD: Mutex<AckHold> = Mutex::new(AckHold {
    holding: false,
    held: Vec::new(),
});

struct AckHold {
    holding: bool,
    held: Vec<BusEvent>,
}

/// Called by the room UI after it lays out a frame.
pub(crate) fn publish(cols: u16, rows: u16, hits: Vec<UiHit>) {
    if let Ok(mut latest) = LATEST.lock() {
        let frame = (cols, rows, hits);
        if latest.as_ref() != Some(&frame) {
            *latest = Some(frame);
        }
    }
}

pub(crate) fn hold_ack(event: BusEvent) -> Option<BusEvent> {
    let Ok(mut hold) = ACK_HOLD.lock() else {
        return Some(event);
    };
    if hold.holding
        && matches!(
            event,
            BusEvent::RoomCreated(_) | BusEvent::AgentAdded(_) | BusEvent::DevFocusRequested { .. }
        )
    {
        hold.held.push(event);
        None
    } else {
        Some(event)
    }
}

pub(crate) fn released_acks() -> Vec<BusEvent> {
    let Ok(mut hold) = ACK_HOLD.lock() else {
        return Vec::new();
    };
    if hold.holding {
        Vec::new()
    } else {
        std::mem::take(&mut hold.held)
    }
}

pub(super) fn set_ack_hold(holding: bool) -> Result<Value, String> {
    let mut hold = ACK_HOLD
        .lock()
        .map_err(|_| "UI acknowledgement hold unavailable")?;
    hold.holding = holding;
    Ok(json!({"holding": holding, "held": hold.held.len()}))
}

pub(super) fn hits_json() -> Result<Value, String> {
    let latest = LATEST.lock().map_err(|_| "UI hits unavailable")?;
    let (cols, rows, hits) = latest
        .as_ref()
        .ok_or("the room UI has not drawn a frame yet")?;
    let hold = ACK_HOLD
        .lock()
        .map_err(|_| "UI acknowledgement hold unavailable")?;
    Ok(json!({
        "size": [cols, rows],
        "hits": hits,
        "holding_acknowledgements": hold.holding,
        "held_acknowledgements": hold.held.len(),
    }))
}
