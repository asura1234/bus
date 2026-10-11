//! Requests between the `bus --dev tui` CLI and its per-session host, and the
//! exit-code contract every verb shares.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(super) const EXIT_OK: i32 = 0;
/// An assertion failed, a wait timed out or a target was not found.
pub(super) const EXIT_FAILED: i32 = 1;
pub(super) const EXIT_USAGE: i32 = 2;
/// No session, or the Bus under the session died.
pub(super) const EXIT_NO_SESSION: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Target {
    Cell {
        row: u16,
        col: u16,
    },
    Text {
        text: String,
        regex: bool,
        nth: Option<usize>,
        rows: Option<(u16, u16)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub(super) enum Command {
    Status,
    Snapshot {
        rows: Option<(u16, u16)>,
    },
    Find {
        text: String,
        regex: bool,
        rows: Option<(u16, u16)>,
    },
    Cells {
        row: u16,
        col: u16,
        width: u16,
        height: u16,
    },
    Type {
        text: String,
    },
    Paste {
        text: String,
    },
    Press {
        keys: Vec<String>,
    },
    Click {
        target: Target,
        button: String,
        mods: String,
        double: bool,
    },
    Drag {
        from: Target,
        to: Target,
        button: String,
        mods: String,
    },
    Scroll {
        target: Target,
        up: bool,
        notches: u16,
        mods: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    WaitText {
        text: String,
        regex: bool,
        gone: bool,
        rows: Option<(u16, u16)>,
        timeout_ms: u64,
    },
    WaitStable {
        stable_ms: u64,
        rows: Option<(u16, u16)>,
        timeout_ms: u64,
    },
    Clipboard,
    Stop,
}

pub(super) fn failure(code: &str, message: impl Into<String>) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message.into()}})
}

/// Exit code for a response: 0 when ok, else by its error code.
pub(super) fn exit_code(response: &Value) -> i32 {
    if response["ok"].as_bool() == Some(true) {
        return EXIT_OK;
    }
    match response["error"]["code"].as_str().unwrap_or("") {
        "usage" => EXIT_USAGE,
        "no_session" | "bus_exited" | "host_unreachable" => EXIT_NO_SESSION,
        _ => EXIT_FAILED,
    }
}
