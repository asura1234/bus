//! The effect recorder: what the scratch Bus asks the operating system to do.
//!
//! Bus reaches the OS through commands it finds on its PATH: `open` for links
//! and files, `osascript` or `terminal-notifier` for desktop notifications and
//! clipboard images, `afplay` for sounds. A driver session puts recording
//! stand-ins for each first on the scratch PATH (the `bus` binary under those
//! names, like the fake providers). They append one JSON line per call to the
//! session's effect log and do nothing else, so a test sees exactly what Bus
//! tried to open or announce and the developer's desktop sees nothing. The host
//! adds terminal notifications (OSC 9, 99 and 777) from the client's output.
//!
//! The clipboard-image stand-in answers Bus's paste from the session's fixture
//! image (`fixture clipboard-image`), never from the real clipboard.
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where the stand-ins append; set in the scratch Bus's environment.
pub(super) const EFFECTS_ENV: &str = "BUS_TUI_EFFECTS";
/// The image the clipboard stand-in serves; set in the scratch environment.
pub(super) const CLIPBOARD_IMAGE_ENV: &str = "BUS_TUI_CLIPBOARD_IMAGE";
pub(super) const COMMANDS: &[&str] = &["open", "osascript", "afplay", "terminal-notifier"];

/// Why a session cannot start on `os`, if it cannot. The stand-ins cover the
/// commands Bus uses on macOS only. Elsewhere Bus opens links, notifies and
/// plays sounds through other programs, which would reach this machine
/// unrecorded, so sessions refuse to start there (Linux is a non-goal for
/// now, Windows comes later).
pub(super) fn unsupported_os(os: &str) -> Option<String> {
    (os != "macos").then(|| {
        format!(
            "driver sessions run on macOS only: the effect recorder cannot stand in for the programs Bus uses on {os} to open links, show notifications and play sounds"
        )
    })
}

/// Runs a stand-in when this process is one; returns its exit code.
pub(super) fn run(command: &str, args: &[String]) -> i32 {
    let (effect, code) = match command {
        "open" => (
            json!({"kind": "open", "target": args.last(), "args": args}),
            0,
        ),
        "afplay" => (
            json!({"kind": "sound", "target": args.last(), "args": args}),
            0,
        ),
        "terminal-notifier" => (
            json!({"kind": "notification", "via": "terminal-notifier",
                   "target": flag(args, "-title"), "body": flag(args, "-message"), "args": args}),
            0,
        ),
        _ => osascript(args),
    };
    record(effect);
    code
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// `osascript -e LINE ... [ARGV]`: a notification, a clipboard-image read,
/// or anything else, which is recorded and succeeds.
fn osascript(args: &[String]) -> (Value, i32) {
    let mut script = Vec::new();
    let mut argv = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-e" {
            script.extend(iter.next().cloned());
        } else if !arg.starts_with('-') {
            argv.push(arg.clone());
        }
    }
    let script = script.join("\n");
    if script.contains("display notification") {
        return (
            json!({"kind": "notification", "via": "osascript", "target": argv.first(),
                   "body": argv.get(1), "args": args}),
            0,
        );
    }
    if script.contains("«class PNGf»") {
        let served = posix_file(&script).is_some_and(|out| {
            std::env::var_os(CLIPBOARD_IMAGE_ENV)
                .map(PathBuf::from)
                .is_some_and(|image| std::fs::copy(image, out).is_ok())
        });
        return (
            json!({"kind": "clipboard_image", "target": posix_file(&script), "served": served}),
            if served { 0 } else { 1 },
        );
    }
    (
        json!({"kind": "osascript", "target": null, "args": args}),
        0,
    )
}

/// The path in `POSIX file "PATH"`.
fn posix_file(script: &str) -> Option<PathBuf> {
    let start = script.find("POSIX file \"")? + "POSIX file \"".len();
    let end = script[start..].find('"')? + start;
    Some(PathBuf::from(&script[start..end]))
}

/// Appends one effect line to the session's effect log.
pub(super) fn record(mut effect: Value) {
    let Some(log) = std::env::var_os(EFFECTS_ENV) else {
        return;
    };
    effect["at_ms"] = json!(now_ms());
    append(Path::new(&log), &effect);
}

pub(super) fn append(log: &Path, effect: &Value) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        // One write per line keeps concurrent appends whole.
        let _ = file.write_all(format!("{effect}\n").as_bytes());
    }
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Every recorded effect, each with its index in the log.
pub(super) fn read(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .enumerate()
        .map(|(index, mut effect)| {
            effect["index"] = json!(index);
            effect
        })
        .collect()
}

/// Whether `effect` is of `kind` and, when given, its target contains `target`.
pub(super) fn matches(effect: &Value, kind: &str, target: Option<&str>) -> bool {
    effect["kind"].as_str() == Some(kind)
        && target.is_none_or(|wanted| {
            effect["target"]
                .as_str()
                .is_some_and(|actual| actual.contains(wanted))
        })
}

/// The OSC numbers of terminal notifications.
pub(super) const NOTIFICATION_OSC: [&str; 3] = ["9", "99", "777"];

/// Where a terminal string (OSC, APC, DCS) whose body starts at `from` ends,
/// and the length of its terminator: BEL, ST (`ESC \`), or any other ESC,
/// which cancels the string and starts the next sequence, as terminals do.
/// None while it is unterminated, an ESC at the very end included.
pub(super) fn string_end(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let at = bytes
        .get(from..)?
        .iter()
        .position(|&b| b == 0x07 || b == 0x1b)?
        + from;
    let terminator = match (bytes[at], bytes.get(at + 1)) {
        (0x07, _) => 1,
        (_, Some(b'\\')) => 2,
        (_, Some(_)) => 0,
        (_, None) => return None,
    };
    Some((at, terminator))
}

/// Collects terminal notifications (OSC 9, 99, 777) across output chunks.
#[derive(Default)]
pub(super) struct OscWatch {
    pending: Vec<u8>,
}

impl OscWatch {
    /// Feeds client output; returns each completed notification's text.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut joined = std::mem::take(&mut self.pending);
        joined.extend_from_slice(bytes);
        let mut i = 0;
        let mut keep_from = joined.len();
        while i + 1 < joined.len() {
            if joined[i] == 0x1b && joined[i + 1] == b']' {
                let Some((end, terminator)) = string_end(&joined, i + 2) else {
                    // An unterminated string waits for the next chunk, within reason.
                    if joined.len() - i < 64 * 1024 {
                        keep_from = i;
                    }
                    break;
                };
                let body = String::from_utf8_lossy(&joined[i + 2..end]).into_owned();
                if let Some((number, text)) = body.split_once(';') {
                    if NOTIFICATION_OSC.contains(&number) {
                        found.push((number.to_owned(), text.to_owned()));
                    }
                }
                i = end + terminator;
            } else {
                i += 1;
            }
        }
        if keep_from == joined.len() && joined.last() == Some(&0x1b) {
            keep_from = joined.len() - 1;
        }
        self.pending = joined[keep_from..].to_vec();
        found
    }
}

#[cfg(test)]
#[path = "tests/effects_test.rs"]
mod tests;
