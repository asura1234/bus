//! `screenshot --real`: the session's exact output bytes replayed into a fresh
//! Ghostty window the driver starts and owns, captured by that window's id,
//! then closed. A real terminal draws what the cell screenshot cannot, above
//! all inline images (Kitty graphics).
//!
//! The mirror never touches the developer's terminals: it is a new Ghostty
//! process with default configuration, found only by its own process id, and
//! the replay drops clipboard writes, notifications and bells first.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Font sizes to try, largest first, until the window holds the whole grid.
const FONT_SIZES: [u8; 3] = [13, 11, 9];
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// Time for Ghostty to draw the replay, images included, before the capture.
const RENDER_WAIT: Duration = Duration::from_millis(900);
const REPLAY_LIFETIME: Duration = Duration::from_secs(120);

/// Drops what the mirror must not act on: OSC 52 (clipboard), OSC 9, 99 and
/// 777 (notifications) and bells outside of strings. Everything else,
/// including Kitty graphics in APC strings, passes unchanged.
pub(super) fn strip_effects(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b if matches!(bytes.get(i + 1), Some(b']' | b'_' | b'P')) => {
                let end = string_end(bytes, i + 2);
                let is_osc = bytes[i + 1] == b']';
                let dropped = is_osc && {
                    let body = &bytes[i + 2..end.min(bytes.len())];
                    let number: Vec<u8> = body
                        .iter()
                        .take_while(|b| b.is_ascii_digit())
                        .copied()
                        .collect();
                    matches!(number.as_slice(), b"52" | b"9" | b"99" | b"777")
                };
                let stop = (end + terminator_len(bytes, end)).min(bytes.len());
                if !dropped {
                    out.extend_from_slice(&bytes[i..stop]);
                }
                i = stop;
            }
            0x07 => i += 1,
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

/// Index of the terminator (BEL or ESC \) of a string starting at `from`.
fn string_end(bytes: &[u8], from: usize) -> usize {
    let mut j = from;
    while j < bytes.len() {
        if bytes[j] == 0x07 || (bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\')) {
            return j;
        }
        j += 1;
    }
    bytes.len()
}

fn terminator_len(bytes: &[u8], at: usize) -> usize {
    match bytes.get(at) {
        Some(0x07) => 1,
        Some(0x1b) => 2,
        _ => 0,
    }
}

/// Entry of the hidden `__replay` verb, which runs inside the mirror window:
/// reports the window's grid, and replays only when it is the session's.
pub(super) fn replay(raw: &Path, ready: &Path, cols: u16, rows: u16) -> i32 {
    let _ = crossterm::terminal::enable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x1b[H\x1b[2J\x1b[3J");
    let _ = stdout.flush();
    let (actual_cols, actual_rows) = settled_size((cols, rows));
    let fits = (actual_cols, actual_rows) == (cols, rows);
    let mut report = json!({"cols": actual_cols, "rows": actual_rows, "replayed": false});
    if fits {
        if let Ok(bytes) = std::fs::read(raw) {
            let _ = stdout.write_all(&strip_effects(&bytes));
            // Leave the cursor hidden, as Bus keeps it, whatever the replay ended on.
            let _ = stdout.write_all(b"\x1b[?25l");
            let _ = stdout.flush();
            report["replayed"] = json!(true);
        }
    }
    let _ = std::fs::write(ready, report.to_string());
    // Stay up, swallowing whatever Ghostty answers, until the driver closes us.
    let deadline = Instant::now() + REPLAY_LIFETIME;
    let mut stdin = std::io::stdin();
    let mut sink = [0u8; 4096];
    while Instant::now() < deadline {
        if stdin.read(&mut sink).unwrap_or(0) == 0 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    0
}

/// The window's grid once Ghostty has applied the requested size: the
/// terminal can start at a default size and grow a moment later.
fn settled_size(wanted: (u16, u16)) -> (u16, u16) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut last = crossterm::terminal::size().unwrap_or((0, 0));
    let mut since = Instant::now();
    while last != wanted && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        let now = crossterm::terminal::size().unwrap_or((0, 0));
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() > Duration::from_millis(1200) {
            break;
        }
    }
    last
}

pub(super) struct Mirror<'a> {
    pub binary: &'a Path,
    pub raw: &'a Path,
    pub work_dir: &'a Path,
    pub out: &'a Path,
    pub cols: u16,
    pub rows: u16,
}

impl Mirror<'_> {
    /// Opens the mirror, captures it and closes it. Errors explain what to do.
    pub fn capture(&self) -> Result<Value, String> {
        let ghostty = ghostty_binary()?;
        let mut last = Value::Null;
        for font_size in FONT_SIZES {
            match self.attempt(&ghostty, font_size)? {
                Ok(captured) => return Ok(captured),
                Err(grid) => last = grid,
            }
        }
        Err(format!(
            "the mirror window could not show the whole {}x{} grid on this screen (got {last}); resize the session smaller and retry",
            self.cols, self.rows
        ))
    }

    /// `Ok(Ok(report))` when captured, `Ok(Err(grid))` when the window's grid
    /// did not match, `Err` when the mirror could not run at all.
    fn attempt(&self, ghostty: &Path, font_size: u8) -> Result<Result<Value, Value>, String> {
        let nonce = format!("{}-{}", std::process::id(), nanos());
        let title = format!("bus-tui-mirror-{nonce}");
        let ready = self.work_dir.join(format!(".mirror-{nonce}.json"));
        let mut child = std::process::Command::new(ghostty)
            .arg("--config-default-files=false")
            .arg(format!("--title={title}"))
            .arg(format!("--window-width={}", self.cols))
            .arg(format!("--window-height={}", self.rows))
            .arg(format!("--font-size={font_size}"))
            .args([
                "--window-save-state=never",
                "--confirm-close-surface=false",
                "--quit-after-last-window-closed=true",
                "-e",
            ])
            .arg(self.binary)
            .args(["--dev", "tui", "__replay", "--raw"])
            .arg(self.raw)
            .arg("--ready")
            .arg(&ready)
            .args(["--size", &format!("{}x{}", self.cols, self.rows)])
            .env_clear()
            .envs(
                ["HOME", "USER", "LOGNAME", "PATH", "TMPDIR", "LANG"]
                    .into_iter()
                    .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start Ghostty for the mirror: {e}"))?;
        let result = self.capture_from(child.id(), &title, &ready, font_size);
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&ready);
        result
    }

    fn capture_from(
        &self,
        pid: u32,
        title: &str,
        ready: &Path,
        font_size: u8,
    ) -> Result<Result<Value, Value>, String> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let report: Value = loop {
            if let Some(report) = std::fs::read_to_string(ready)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
            {
                break report;
            }
            if Instant::now() > deadline {
                return Err("the mirror window never started its replay".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        if report["replayed"].as_bool() != Some(true) {
            return Ok(Err(json!([report["cols"], report["rows"]])));
        }
        std::thread::sleep(RENDER_WAIT);
        let window = find_window(pid, title)?;
        let status = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-o", "-l", &window.to_string()])
            .arg(self.out)
            .status()
            .map_err(|e| format!("screencapture: {e}"))?;
        if !status.success() || !self.out.exists() {
            return Err(format!(
                "screencapture could not capture the mirror window {window}; grant Screen Recording to the terminal running the driver"
            ));
        }
        Ok(Ok(
            json!({"path": self.out, "window_id": window, "grid": [self.cols, self.rows],
                     "font_size": font_size}),
        ))
    }
}

fn ghostty_binary() -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("screenshot --real needs macOS and Ghostty for now; use screenshot".into());
    }
    let mut candidates = vec![PathBuf::from(
        "/Applications/Ghostty.app/Contents/MacOS/ghostty",
    )];
    if let Some(home) = std::env::home_dir() {
        candidates.push(home.join("Applications/Ghostty.app/Contents/MacOS/ghostty"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "screenshot --real needs Ghostty in /Applications; use screenshot".into())
}

/// The on-screen window of exactly this Ghostty process. Its title is
/// checked when macOS shows titles (Screen Recording permission).
fn find_window(pid: u32, title: &str) -> Result<u64, String> {
    let script = format!(
        "ObjC.import('CoreGraphics');\
         var w = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo($.kCGWindowListOptionAll, 0)));\
         JSON.stringify(w.filter(function (x) {{ return x.kCGWindowOwnerPID === {pid} && x.kCGWindowLayer === 0 && x.kCGWindowIsOnscreen; }})\
           .map(function (x) {{ return {{id: x.kCGWindowNumber, title: x.kCGWindowName === undefined ? null : x.kCGWindowName}}; }}));"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = std::process::Command::new("/usr/bin/osascript")
            .args(["-l", "JavaScript", "-e", &script])
            .output()
            .map_err(|e| format!("cannot list windows: {e}"))?;
        let windows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
        let ours: Vec<&Value> = windows
            .iter()
            .filter(|w| w["title"].is_null() || w["title"].as_str() == Some(title))
            .collect();
        if let [window] = ours.as_slice() {
            if let Some(id) = window["id"].as_u64() {
                return Ok(id);
            }
        }
        if Instant::now() > deadline {
            return Err(format!(
                "found {} mirror windows for Ghostty process {pid}, expected exactly one",
                ours.len()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/mirror_test.rs"]
mod tests;
