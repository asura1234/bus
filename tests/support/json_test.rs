use super::*;
use std::io::{Read, Write};

pub struct JsonLineReader {
    stream: UnixStream,
    buf: Vec<u8>,
}

impl JsonLineReader {
    pub fn connect(socket_path: &Path) -> Self {
        Self {
            stream: UnixStream::connect(socket_path).unwrap(),
            buf: Vec::new(),
        }
    }

    pub fn send_line(&mut self, json: &str) {
        self.stream.write_all(json.as_bytes()).unwrap();
        self.stream.write_all(b"\n").unwrap();
        self.stream.flush().unwrap();
    }

    pub fn read_json_line(&mut self, timeout: Duration) -> serde_json::Value {
        self.try_read_json_line(timeout)
            .unwrap_or_else(|| panic!("timed out waiting for json line"))
    }

    pub fn try_read_json_line(&mut self, timeout: Duration) -> Option<serde_json::Value> {
        let deadline = Instant::now() + timeout;
        self.stream.set_nonblocking(true).unwrap();

        loop {
            if Instant::now() >= deadline {
                self.stream.set_nonblocking(false).unwrap();
                return None;
            }

            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8(self.buf.drain(..=pos).collect()).unwrap();
                self.stream.set_nonblocking(false).unwrap();
                return Some(serde_json::from_str(&line).unwrap());
            }

            let mut bytes = [0u8; 256];
            match self.stream.read(&mut bytes) {
                Ok(0) => panic!("stream closed while waiting for json line"),
                Ok(n) => self.buf.extend_from_slice(&bytes[..n]),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("failed to read json line: {err}"),
            }
        }
    }
}

pub fn send_request(socket_path: &Path, json: &str) -> serde_json::Value {
    let mut reader = JsonLineReader::connect(socket_path);
    reader.send_line(json);
    reader.read_json_line(Duration::from_secs(5))
}

pub fn open_subscription(socket_path: &Path, json: &str) -> JsonLineReader {
    let mut reader = JsonLineReader::connect(socket_path);
    reader.send_line(json);
    reader
}

#[cfg(not(target_os = "macos"))]
pub fn wait_for_event(
    reader: &mut JsonLineReader,
    expected: &str,
    timeout: Duration,
) -> serde_json::Value {
    wait_for_event_matching(reader, expected, timeout, |_| true)
}

#[cfg(not(target_os = "macos"))]
pub fn wait_for_event_matching<F>(
    reader: &mut JsonLineReader,
    expected: &str,
    timeout: Duration,
    mut matches: F,
) -> serde_json::Value
where
    F: FnMut(&serde_json::Value) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let value = reader.read_json_line(remaining.max(Duration::from_millis(1)));
        if value["event"] == expected && matches(&value) {
            return value;
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn wait_for_events(
    reader: &mut JsonLineReader,
    expected: &[&str],
    timeout: Duration,
) -> Vec<serde_json::Value> {
    let deadline = Instant::now() + timeout;
    let mut remaining = expected.to_vec();
    let mut events = Vec::new();
    while !remaining.is_empty() {
        let remaining_timeout = deadline.saturating_duration_since(Instant::now());
        let value = reader.read_json_line(remaining_timeout.max(Duration::from_millis(1)));
        let Some(event) = value["event"].as_str() else {
            continue;
        };
        if let Some(index) = remaining.iter().position(|expected| *expected == event) {
            remaining.remove(index);
            events.push(value);
        }
    }
    events
}

#[cfg(not(target_os = "macos"))]
pub fn event_by_kind<'a>(events: &'a [serde_json::Value], kind: &str) -> &'a serde_json::Value {
    events
        .iter()
        .find(|event| event["event"] == kind)
        .unwrap_or_else(|| panic!("missing event {kind}"))
}

pub fn start_pane_command(socket_path: &Path, pane_id: &str, command: &str) {
    let sent = send_request(
        socket_path,
        &serde_json::json!({
            "id": "start_command",
            "method": "pane.send_input",
            "params": { "pane_id": pane_id, "text": command, "keys": ["Enter"] },
        })
        .to_string(),
    );
    assert_eq!(sent["result"]["type"], "ok", "{sent}");
}

pub fn wait_for_pane_agent(socket_path: &Path, pane_id: &str, agent: &str, status: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let pane = send_request(
            socket_path,
            &format!(
                r#"{{"id":"pane_agent","method":"pane.get","params":{{"pane_id":"{pane_id}"}}}}"#
            ),
        );
        if pane["result"]["pane"]["agent"] == agent
            && pane["result"]["pane"]["agent_status"] == status
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{agent} was never detected as {status}: {pane}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}
