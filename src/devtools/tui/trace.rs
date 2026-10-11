//! The run folder: every request and its result (`trace.ndjson`) and the
//! terminal's input and output as an asciicast v2 recording (`pty.cast`).
use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) struct Trace {
    started: Instant,
    cast: BufWriter<File>,
    requests: BufWriter<File>,
}

impl Trace {
    pub fn open(run_dir: &Path, cols: u16, rows: u16) -> std::io::Result<Self> {
        let mut cast = BufWriter::new(File::create(run_dir.join("pty.cast"))?);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        writeln!(
            cast,
            "{}",
            json!({"version": 2, "width": cols, "height": rows, "timestamp": timestamp,
                   "env": {"TERM": "xterm-256color"}})
        )?;
        Ok(Self {
            started: Instant::now(),
            cast,
            requests: BufWriter::new(File::create(run_dir.join("trace.ndjson"))?),
        })
    }

    fn event(&mut self, kind: &str, data: &str) {
        let at = self.started.elapsed().as_secs_f64();
        let _ = writeln!(self.cast, "{}", json!([at, kind, data]));
    }

    pub fn output(&mut self, bytes: &[u8]) {
        self.event("o", &String::from_utf8_lossy(bytes));
    }

    pub fn input(&mut self, bytes: &[u8]) {
        self.event("i", &String::from_utf8_lossy(bytes));
        let _ = self.cast.flush();
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.event("r", &format!("{cols}x{rows}"));
    }

    pub fn record(&mut self, request: &str, response: &Value, took: Duration) {
        let request: Value = serde_json::from_str(request.trim()).unwrap_or(Value::Null);
        let _ = writeln!(
            self.requests,
            "{}",
            json!({"at_ms": self.started.elapsed().as_millis() as u64,
                   "took_ms": took.as_millis() as u64,
                   "request": request,
                   "ok": response["ok"],
                   "error": response.get("error")})
        );
        let _ = self.requests.flush();
        let _ = self.cast.flush();
    }

    pub fn flush(&mut self) {
        let _ = self.requests.flush();
        let _ = self.cast.flush();
    }
}
