//! Dev variant only: the room UI's clickable regions and acknowledgement hold,
//! transported by local clients when coordination lives in the native server.
use super::BusEvent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

/// One clickable region: a stable name such as `room:review`, `new-agent` or
/// `color-blind`, and its cells.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UiHit {
    pub name: String,
    pub row: u16,
    pub col: u16,
    pub width: u16,
    pub height: u16,
}

type Frame = (u16, u16, Vec<UiHit>);

static LATEST: Mutex<Option<Frame>> = Mutex::new(None);
static CLIENT_HELD: AtomicUsize = AtomicUsize::new(0);
static ACK_HOLD: Mutex<AckHold> = Mutex::new(AckHold {
    holding: false,
    held: Vec::new(),
});

struct AckHold {
    holding: bool,
    held: Vec<BusEvent>,
}

#[derive(Serialize, Deserialize)]
struct ClientState {
    frame: Option<Frame>,
    form_generation: u64,
    held: usize,
}

/// Only the local attached viewer publishes UI diagnostics. Remote viewers
/// neither overwrite this machine's hit map nor inherit its dev controls.
pub(crate) fn client_state() -> Value {
    json!(ClientState {
        frame: LATEST.lock().ok().and_then(|latest| latest.clone()),
        form_generation: super::super::current_form_generation(),
        held: ACK_HOLD.lock().map(|hold| hold.held.len()).unwrap_or(0),
    })
}

pub(super) fn receive_client_state(value: &Value) {
    let Ok(state) = serde_json::from_value::<ClientState>(value.clone()) else {
        return;
    };
    if let Ok(mut latest) = LATEST.lock() {
        *latest = state.frame;
    }
    super::super::publish_form_generation(state.form_generation);
    CLIENT_HELD.store(state.held, Ordering::Relaxed);
}

pub(super) fn ack_holding() -> bool {
    ACK_HOLD.lock().is_ok_and(|hold| hold.holding)
}

pub(crate) fn receive_ack_hold(holding: bool) {
    if let Ok(mut hold) = ACK_HOLD.lock() {
        hold.holding = holding;
    }
}

fn held_count(hold: &AckHold) -> usize {
    if crate::messaging::service::is_running() {
        CLIENT_HELD.load(Ordering::Relaxed)
    } else {
        hold.held.len()
    }
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
            BusEvent::RoomCreated { .. }
                | BusEvent::AgentAdded { .. }
                | BusEvent::DevFocusRequested { .. }
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
    Ok(json!({"holding": holding, "held": held_count(&hold)}))
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
        "held_acknowledgements": held_count(&hold),
    }))
}
