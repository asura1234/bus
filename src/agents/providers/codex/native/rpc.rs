//! Local app-server RPC. Connection failure and an uncertain mutation differ.
use serde_json::{json, Value};
use std::{io, os::unix::net::UnixStream, path::Path, time::Instant};
use tungstenite::WebSocket;

/// Bus quotes an attachment as one double-quoted token and only unescapes
/// `\\` and `\"`. JSON string decoding treats `\b` and the other JSON escapes
/// as different characters, so it must not parse this line.
fn pasted_image_path(line: &str) -> io::Result<String> {
    let tokens = crate::agents::providers::launch::parse_path_tokens(line).map_err(|error| {
        io::Error::other(format!(
            "Codex native image path is not a quoted token: {error}"
        ))
    })?;
    match tokens.as_slice() {
        [path] => Ok(path.clone()),
        _ => Err(io::Error::other(
            "Codex native image path is not one quoted token",
        )),
    }
}

#[derive(Debug)]
struct PreflightRefusal {
    source: io::Error,
}

impl std::fmt::Display for PreflightRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Codex native preflight refused before input")
    }
}

impl std::error::Error for PreflightRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn preflight_refusal(source: io::Error) -> io::Error {
    tracing::info!(
        event = "bus.codex.native_preflight",
        error = %source,
        "Codex native preflight refused before input"
    );
    io::Error::new(io::ErrorKind::WouldBlock, PreflightRefusal { source })
}

fn lost_response(method: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("Codex native {method}: {error}"))
}

pub(super) struct Rpc {
    socket: WebSocket<UnixStream>,
    next_id: u64,
    deadline: Instant,
}

impl Rpc {
    pub(super) fn connect(path: &Path, deadline: Instant) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let timeout = deadline.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Codex native deadline elapsed",
            ));
        }
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(8 * 1024 * 1024))
            .max_frame_size(Some(8 * 1024 * 1024));
        let (socket, _) =
            tungstenite::client::client_with_config("ws://localhost/", stream, Some(config))
                .map_err(io::Error::other)?;
        let mut rpc = Self {
            socket,
            next_id: 0,
            deadline,
        };
        rpc.call(
            "initialize",
            json!({"clientInfo":{"name":"bus_native_input","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
        )?;
        rpc.socket
            .send(tungstenite::Message::Text(
                json!({"method":"initialized"}).to_string().into(),
            ))
            .map_err(io::Error::other)?;
        Ok(rpc)
    }

    pub(super) fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = deadline;
    }

    fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.timeout()
            .map_err(|error| lost_response(method, error))?;
        // From here a transport error must never cause fallback or replay.
        self.socket
            .send(tungstenite::Message::Text(
                json!({"id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .map_err(|error| lost_response(method, io::Error::other(error)))?;
        loop {
            self.timeout()
                .map_err(|error| lost_response(method, error))?;
            let frame = self
                .socket
                .read()
                .map_err(|error| lost_response(method, io::Error::other(error)))?;
            let tungstenite::Message::Text(text) = frame else {
                if frame.is_close() {
                    return Err(lost_response(
                        method,
                        io::Error::new(io::ErrorKind::BrokenPipe, "Codex app-server disconnected"),
                    ));
                }
                continue;
            };
            let value: Value = serde_json::from_str(&text)
                .map_err(|error| lost_response(method, io::Error::other(error)))?;
            // Server requests use an independent ID namespace. The TUI handles
            // approvals; only a response may settle this client's admission.
            if value.get("method").is_some() || value.get("id") != Some(&json!(id)) {
                continue;
            }
            if value.get("error").is_some() {
                // The queue may auto-dispatch between add and explicit start.
                // Match only the exact owned item and protocol rejection code.
                if method == "thread/queue/start"
                    && value["error"]["code"] == -32600
                    && params["queuedSubmissionId"].as_str().is_some_and(|id| {
                        value["error"]["message"] == format!("queued submission not found: {id}")
                    })
                {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Codex queued input already dispatched",
                    ));
                }
                // Do not include provider text: errors can contain prompt data.
                return Err(io::Error::other(format!(
                    "Codex native {method} rejected (code {})",
                    value["error"]["code"]
                )));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| io::Error::other("Invalid Codex native response"));
        }
    }

    fn timeout(&self) -> io::Result<()> {
        let timeout = self.deadline.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Codex native deadline elapsed",
            ));
        }
        self.socket.get_ref().set_read_timeout(Some(timeout))?;
        self.socket.get_ref().set_write_timeout(Some(timeout))
    }

    fn active_turn(&mut self, thread: &str, interrupt: bool) -> io::Result<Option<String>> {
        let result = self.call(
            "thread/read",
            json!({"threadId":thread,"includeTurns":false}),
        )?;
        let owned = &result["thread"];
        if owned["id"] != thread {
            return Err(io::Error::other("Codex native thread identity mismatch"));
        }
        match owned["status"]["type"].as_str() {
            Some("idle") => Ok(None),
            Some("active")
                if owned["status"]["activeFlags"]
                    .as_array()
                    .is_some_and(|flags| interrupt || flags.is_empty()) =>
            {
                let history = self.call(
                    "thread/turns/list",
                    json!({"threadId":thread,"limit":1,"sortDirection":"desc","itemsView":"notLoaded"}),
                )?;
                let turn = history["data"]
                    .as_array()
                    .and_then(|turns| turns.first())
                    .ok_or_else(|| io::Error::other("Codex native active turn is unavailable"))?;
                if turn["status"] != "inProgress" {
                    return Err(io::Error::other("Codex native active turn changed"));
                }
                turn["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(|id| Some(id.to_owned()))
                    .ok_or_else(|| io::Error::other("Codex native active turn has no identity"))
            }
            _ => Err(io::Error::other(
                "Codex native thread is blocked or unavailable",
            )),
        }
    }

    pub(super) fn prompt(
        &mut self,
        thread: &str,
        text: &str,
        steer: bool,
        message: &str,
        expected_turn: Option<&str>,
    ) -> io::Result<Value> {
        let (thread, active) = (|| -> io::Result<_> {
            let thread = if thread.is_empty() {
                let loaded = self.call("thread/loaded/list", json!({"limit":2}))?;
                let threads = loaded["data"]
                    .as_array()
                    .ok_or_else(|| io::Error::other("Codex loaded thread list is invalid"))?;
                if threads.len() != 1 || !loaded["nextCursor"].is_null() {
                    return Err(io::Error::other(
                        "Codex initial input requires exactly one loaded TUI thread",
                    ));
                }
                threads[0]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| io::Error::other("Codex loaded thread has no identity"))?
                    .to_owned()
            } else {
                thread.to_owned()
            };
            let active = self.active_turn(&thread, false)?;
            Ok((thread, active))
        })()
        .map_err(preflight_refusal)?;
        if steer
            && (expected_turn.is_none()
                || active
                    .as_deref()
                    .is_some_and(|turn| Some(turn) != expected_turn))
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Codex active turn changed before steering",
            ));
        }
        if active.is_some() && !steer {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Codex native turn is already active",
            ));
        }
        let (images, text) = super::super::hooks::image_paste_parts(text);
        let mut input = Vec::with_capacity(images.len() + usize::from(!text.is_empty()));
        for image in images {
            input.push(json!({"type":"localImage","path":pasted_image_path(image)?}));
        }
        if !text.is_empty() {
            input.push(json!({"type":"text","text":text}));
        }
        let mut params = json!({"threadId":thread,"input":input,"clientUserMessageId":message});
        let method = if let Some(turn) = active {
            params["expectedTurnId"] = json!(turn);
            "turn/steer"
        } else {
            "thread/queue/add"
        };
        tracing::info!(
            event = "bus.codex.native_request",
            api_request_id = message,
            thread_id = thread,
            expected_turn_id = expected_turn,
            method,
            "Submitting Codex native input"
        );
        let admitted = self.call(method, params)?;
        if method == "thread/queue/add" {
            let queued = admitted["queuedSubmission"]["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| io::Error::other("Codex admitted queue receipt has no identity"))?;
            // Interrupt pauses Codex's queue. Wake exactly the receipted entry;
            // turn/start could accidentally steer a racing developer turn.
            tracing::info!(
                event = "bus.codex.native_request",
                api_request_id = message,
                thread_id = thread,
                method = "thread/queue/start",
                "Starting admitted Codex native input"
            );
            match self.call(
                "thread/queue/start",
                json!({"threadId":thread,"queuedSubmissionId":queued}),
            ) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
                Ok(_) => {}
            }
        }
        Ok(admitted)
    }

    // Keep turn metadata bounded; neither interrupt nor confirmation loads items.
    fn interrupt_state(&mut self, thread: &str) -> io::Result<(Option<String>, bool)> {
        let result = self.call(
            "thread/read",
            json!({"threadId":thread,"includeTurns":false}),
        )?;
        if result["thread"]["id"] != thread {
            return Err(io::Error::other("Codex native thread identity mismatch"));
        }
        let idle = match result["thread"]["status"]["type"].as_str() {
            Some("idle") => true,
            Some("active") if result["thread"]["status"]["activeFlags"].is_array() => false,
            _ => {
                return Err(io::Error::other(
                    "Codex native interrupt status unavailable",
                ))
            }
        };
        let history = self.call(
            "thread/turns/list",
            json!({"threadId":thread,"limit":1,"sortDirection":"desc","itemsView":"notLoaded"}),
        )?;
        let turns = history["data"]
            .as_array()
            .filter(|turns| turns.len() <= 1)
            .ok_or_else(|| io::Error::other("Codex native interrupt turn metadata invalid"))?;
        let Some(turn) = turns.first() else {
            return if idle {
                Ok((None, true))
            } else {
                Err(io::Error::other("Codex native active turn missing"))
            };
        };
        let id = turn["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| io::Error::other("Codex native interrupt turn identity missing"))?;
        let ended = match turn["status"].as_str() {
            Some("completed" | "interrupted" | "failed") => true,
            Some("inProgress") => false,
            _ => {
                return Err(io::Error::other(
                    "Codex native interrupt turn status invalid",
                ))
            }
        };
        Ok((Some(id.to_owned()), idle && ended))
    }

    fn confirm_empty_terminals(&mut self, thread: &str) -> io::Result<()> {
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            let result = self.call(
                "thread/backgroundTerminals/list",
                json!({"threadId":thread,"limit":64,"cursor":cursor}),
            )?;
            if !result["data"]
                .as_array()
                .is_some_and(|data| data.is_empty())
            {
                return Err(io::Error::other(
                    "Codex native interrupt has live or unknown terminals",
                ));
            }
            match result.get("nextCursor") {
                Some(Value::Null) => return Ok(()),
                Some(Value::String(next)) if !next.is_empty() && seen.insert(next.clone()) => {
                    cursor = Some(next.clone());
                }
                _ => {
                    return Err(io::Error::other(
                        "Codex native terminal enumeration incomplete",
                    ))
                }
            }
        }
        Err(io::Error::other(
            "Codex native terminal enumeration exceeded page bound",
        ))
    }

    pub(super) fn interrupt(&mut self, thread: &str) -> io::Result<Option<String>> {
        let (target, ended) = self.interrupt_state(thread)?;
        if !ended {
            let turn = target
                .as_ref()
                .ok_or_else(|| io::Error::other("Codex native active turn missing"))?;
            tracing::info!(
                event = "bus.codex.native_request",
                thread_id = thread,
                turn_id = turn,
                method = "turn/interrupt",
                "Interrupting Codex native turn"
            );
            self.call("turn/interrupt", json!({"threadId":thread,"turnId":turn}))?;
        }
        loop {
            let (current, ended) = self.interrupt_state(thread)?;
            if current != target {
                return Err(io::Error::other("Codex native interrupted turn changed"));
            }
            if ended {
                break;
            }
            self.timeout()?;
            std::thread::sleep(
                std::time::Duration::from_millis(10)
                    .min(self.deadline.saturating_duration_since(Instant::now())),
            );
        }
        self.confirm_empty_terminals(thread)?;
        let (current, ended) = self.interrupt_state(thread)?;
        if current != target || !ended {
            return Err(io::Error::other(
                "Codex native turn changed during interrupt confirmation",
            ));
        }
        Ok(target)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{os::unix::net::UnixListener, time::Duration};

    // Native external dependency is a wire fixture, not a fake Bus component.
    // Removing expectedTurnId, changing text bytes, or resuming a second writer
    // breaks the observed requests below.

    fn server(
        active: bool,
        lost_ack: bool,
    ) -> (std::path::PathBuf, std::thread::JoinHandle<Vec<Value>>) {
        server_for("owned-thread", active, lost_ack)
    }

    pub(crate) fn server_for(
        thread: &'static str,
        active: bool,
        lost_ack: bool,
    ) -> (std::path::PathBuf, std::thread::JoinHandle<Vec<Value>>) {
        server_interleaved(thread, active, lost_ack, false)
    }

    fn server_interleaved(
        thread: &'static str,
        active: bool,
        lost_ack: bool,
        interleaved: bool,
    ) -> (std::path::PathBuf, std::thread::JoinHandle<Vec<Value>>) {
        let dir = crate::utils::test_temp::unique_temp_path("codex-native-rpc");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut calls = Vec::new();
            let mut interrupted = false;
            let mut enumerated = false;
            loop {
                let message = match ws.read() {
                    Ok(message) => message,
                    Err(_) => break,
                };
                let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
                calls.push(value.clone());
                let Some(id) = value.get("id") else { continue };
                let method = value["method"].as_str().unwrap();
                let result = match method {
                    "initialize" => json!({"userAgent":"fixture"}),
                    "thread/loaded/list" => json!({"data":[thread],"nextCursor":null}),
                    "thread/read" => {
                        json!({"thread":{"id":thread, "status":if active && !interrupted {json!({"type":"active","activeFlags":[]})} else {json!({"type":"idle"})}, "turns":if active && !interrupted {json!([{"id":"busy-turn","status":"inProgress","items":[]}])} else {json!([])}}})
                    }
                    "thread/turns/list" => {
                        json!({"data":if active {json!([{"id":"busy-turn","status":if interrupted {"interrupted"} else {"inProgress"},"items":[]}])} else {json!([])},"nextCursor":null})
                    }
                    "turn/interrupt" => {
                        interrupted = true;
                        json!({})
                    }
                    "thread/backgroundTerminals/list" => {
                        enumerated = true;
                        json!({"data":[],"nextCursor":null})
                    }
                    "thread/queue/add" | "turn/steer" | "thread/queue/start" => {
                        if lost_ack {
                            break;
                        }
                        if method == "thread/queue/add" {
                            json!({"queuedSubmission":{"id":"native-queue","clientUserMessageId":value["params"]["clientUserMessageId"]}})
                        } else {
                            json!({"turn":{"id":"native-turn","status":"inProgress"}})
                        }
                    }
                    other => panic!("unexpected mutating/attach operation {other}"),
                };
                if interleaved && method == "thread/queue/add" {
                    ws.send(tungstenite::Message::Text(
                        json!({"id":id,"method":"item/commandExecution/requestApproval","params":{"threadId":thread,"turnId":"native-turn","itemId":"fixture-command","command":"fixture-command","cwd":"/fixture"}}).to_string().into(),
                    )).unwrap();
                }
                ws.send(tungstenite::Message::Text(
                    json!({"id":id,"result":result}).to_string().into(),
                ))
                .unwrap();
                if method == "turn/steer"
                    || method == "thread/queue/start"
                    || (enumerated && method == "thread/turns/list")
                {
                    break;
                }
            }
            let _ = std::fs::remove_dir_all(dir);
            calls
        });
        (path, handle)
    }

    #[test]
    fn codex_native_idle_uses_existing_runtime_and_preserves_exact_text() {
        let (path, handle) = server(false, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.prompt(
            "owned-thread",
            "保留\n  two\n\tthree  ",
            false,
            "bus-msg-7",
            None,
        )
        .unwrap();
        let calls = handle.join().unwrap();
        let prompt = calls
            .iter()
            .find(|v| v["method"] == "thread/queue/add")
            .unwrap();
        assert_eq!(calls.last().unwrap()["method"], "thread/queue/start");
        assert_eq!(prompt["params"]["threadId"], "owned-thread");
        assert_eq!(
            prompt["params"]["input"],
            json!([{"type":"text","text":"保留\n  two\n\tthree  "}])
        );
        assert_eq!(prompt["params"]["clientUserMessageId"], "bus-msg-7");
    }

    #[test]
    fn codex_native_quoted_image_path_keeps_a_literal_backslash() {
        let (path, handle) = server(false, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        // Bus double quotes keep a backslash that is not `\\` or `\"`.
        rpc.prompt(
            "owned-thread",
            "\"/tmp/plain.png\"\n\"/tmp/foo\\bar.png\"",
            false,
            "image-backslash",
            None,
        )
        .unwrap();
        let calls = handle.join().unwrap();
        let input = &calls
            .iter()
            .find(|call| call["method"] == "thread/queue/add")
            .unwrap()["params"]["input"];
        assert_eq!(input[0]["path"], "/tmp/plain.png");
        assert_eq!(input[1]["path"], "/tmp/foo\\bar.png");
        assert_eq!(input.as_array().unwrap().len(), 2);
    }

    #[test]
    fn codex_native_preflight_refusal_keeps_the_source_error() {
        let (path, handle) = server(false, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.set_deadline(Instant::now());
        let error = rpc
            .prompt("owned-thread", "late", false, "preflight", None)
            .unwrap_err();
        drop(rpc);
        let _ = handle.join();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            error.to_string(),
            "Codex native preflight refused before input"
        );
        let source =
            std::error::Error::source(&error).expect("preflight refusal must keep the source");
        let source = source.to_string();
        assert!(
            source.contains("deadline elapsed"),
            "deadline was collapsed: {source}"
        );
        assert!(
            source.contains("thread/read"),
            "preflight source must name the call: {source}"
        );
    }

    #[test]
    fn codex_native_steer_reads_only_the_latest_turn_without_items() {
        let (path, handle) = server(true, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.prompt("owned-thread", "steer", true, "busy-id", Some("busy-turn"))
            .unwrap();
        let calls = handle.join().unwrap();
        let query = calls
            .iter()
            .find(|call| call["method"] == "thread/turns/list")
            .unwrap();
        assert_eq!(
            query["params"],
            json!({"threadId":"owned-thread","limit":1,"sortDirection":"desc","itemsView":"notLoaded"})
        );
        assert!(!calls
            .iter()
            .any(|call| call["method"] == "thread/read" && call["params"]["includeTurns"] == true));
    }

    #[test]
    fn codex_native_server_request_id_collision_does_not_consume_our_admission_ack() {
        let (path, handle) = server_interleaved("owned-thread", false, false, true);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        let result = rpc.prompt("owned-thread", "once", false, "interleaved", None);
        let calls = handle.join().unwrap();
        assert!(
            result.is_ok(),
            "a server request is not our response: {result:?}"
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["method"] == "thread/queue/add")
                .count(),
            1
        );
    }

    #[test]
    fn codex_native_busy_steer_is_bound_to_the_live_turn() {
        let (path, handle) = server(true, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.prompt(
            "owned-thread",
            "busy payload",
            true,
            "bus-msg-8",
            Some("busy-turn"),
        )
        .unwrap();
        let calls = handle.join().unwrap();
        let prompt = calls.last().unwrap();
        assert_eq!(prompt["method"], "turn/steer");
        assert_eq!(prompt["params"]["expectedTurnId"], "busy-turn");
        assert_eq!(prompt["params"]["input"][0]["text"], "busy payload");
        assert!(!calls.iter().any(|call| call["method"] == "thread/resume"));
    }

    #[test]
    fn codex_native_lost_ack_never_repeats_a_mutation() {
        let (path, handle) = server(false, true);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        let error = rpc
            .prompt("owned-thread", "once", false, "bus-msg-9", None)
            .unwrap_err();
        assert!(
            error.to_string().contains("thread/queue/add"),
            "lost admission must name its method: {error}"
        );
        assert!(!error.to_string().contains("thread/queue/start"));
        let calls = handle.join().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["method"] == "thread/queue/add")
                .count(),
            1
        );
        let (path, handle) = server(true, true);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        let error = rpc
            .prompt("owned-thread", "once", true, "bus-msg-9", Some("busy-turn"))
            .unwrap_err();
        assert!(
            error.to_string().contains("turn/steer"),
            "lost steer must name its method: {error}"
        );
        drop(rpc);
        let calls = handle.join().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["method"] == "turn/steer")
                .count(),
            1
        );
    }
    #[test]
    fn codex_native_first_input_uses_the_only_loaded_tui_thread_without_resuming() {
        let (path, handle) = server(false, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.prompt("", "first input", false, "bus-first", None)
            .unwrap();
        let calls = handle.join().unwrap();
        assert_eq!(calls.last().unwrap()["params"]["threadId"], "owned-thread");
        assert!(!calls.iter().any(|call| matches!(
            call["method"].as_str(),
            Some("thread/start" | "thread/resume")
        )));
    }
    #[test]
    fn codex_native_stale_steering_never_joins_a_newer_turn() {
        let (path, handle) = server(true, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        let error = rpc
            .prompt(
                "owned-thread",
                "do not join",
                true,
                "bus-stale",
                Some("old-turn"),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(rpc);
        let calls = handle.join().unwrap();
        assert!(!calls.iter().any(|call| call["method"]
            .as_str()
            .is_some_and(|method| method.starts_with("turn/"))));
    }

    #[test]
    fn codex_native_interrupt_targets_the_live_owned_turn() {
        let (path, handle) = server(true, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        assert_eq!(
            rpc.interrupt("owned-thread").unwrap().as_deref(),
            Some("busy-turn")
        );
        let calls = handle.join().unwrap();
        let interrupts: Vec<_> = calls
            .iter()
            .filter(|call| call["method"] == "turn/interrupt")
            .collect();
        assert_eq!(interrupts.len(), 1);
        assert_eq!(
            interrupts[0]["params"],
            json!({"threadId":"owned-thread","turnId":"busy-turn"})
        );
        assert!(calls
            .iter()
            .any(|call| call["method"] == "thread/backgroundTerminals/list"));
    }
    fn queued_server(
        start_error: Option<Value>,
    ) -> (std::path::PathBuf, std::thread::JoinHandle<Vec<Value>>) {
        let dir = crate::utils::test_temp::unique_temp_path("codex-paused-queue");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut calls = Vec::new();
            while let Ok(frame) = ws.read() {
                let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                calls.push(value.clone());
                let Some(id) = value.get("id") else { continue };
                let method = value["method"].as_str().unwrap();
                let result = match method {
                    "initialize" => json!({"userAgent":"fixture"}),
                    "thread/read" => {
                        json!({"thread":{"id":"owned-thread","status":{"type":"idle"}}})
                    }
                    "thread/queue/add" => {
                        json!({"queuedSubmission":{"id":"owned-queue","clientUserMessageId":"after-interrupt"}})
                    }
                    "thread/queue/start" => {
                        if let Some(error) = &start_error {
                            if !error.is_null() {
                                ws.send(tungstenite::Message::Text(
                                    json!({"id":id,"error":error}).to_string().into(),
                                ))
                                .unwrap();
                            }
                            break;
                        }
                        json!({"turn":{"id":"after-interrupt-turn","status":"inProgress"}})
                    }
                    other => panic!("unexpected native operation {other}"),
                };
                ws.send(tungstenite::Message::Text(
                    json!({"id":id,"result":result}).to_string().into(),
                ))
                .unwrap();
                if method == "thread/queue/start" {
                    break;
                }
            }
            std::fs::remove_dir_all(dir).unwrap();
            calls
        });
        (path, handle)
    }

    #[test]
    fn codex_native_paused_queue_starts_only_the_receipted_submission() {
        let (path, server) = queued_server(None);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        let result = rpc.prompt(
            "owned-thread",
            "after interrupt",
            false,
            "after-interrupt",
            None,
        );
        drop(rpc);
        let calls = server.join().unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(calls.last().unwrap()["method"], "thread/queue/start");
        assert_eq!(
            calls.last().unwrap()["params"],
            json!({"threadId":"owned-thread","queuedSubmissionId":"owned-queue"})
        );
        assert_eq!(
            calls
                .iter()
                .filter(|v| v["method"] == "thread/queue/add")
                .count(),
            1
        );
        assert!(!calls.iter().any(|v| v["method"] == "turn/start"));
    }

    #[test]
    fn codex_native_auto_consumed_queue_is_never_reenqueued() {
        let (path, server) = queued_server(Some(
            json!({"code":-32600,"message":"queued submission not found: owned-queue"}),
        ));
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        let result = rpc.prompt(
            "owned-thread",
            "after interrupt",
            false,
            "after-interrupt",
            None,
        );
        drop(rpc);
        let calls = server.join().unwrap();
        assert!(
            result.is_ok(),
            "admitted queue item may already be consumed: {result:?}"
        );
        assert_eq!(calls.last().unwrap()["method"], "thread/queue/start");
        assert_eq!(
            calls
                .iter()
                .filter(|v| v["method"] == "thread/queue/add")
                .count(),
            1
        );
    }

    #[test]
    fn codex_native_queue_wakeup_failure_keeps_the_existing_admission_uncertain() {
        for failure in [
            Value::Null,
            json!({"code":-32000,"message":"unavailable"}),
            json!({"code":-32600,"message":"thread already has an active or pending turn"}),
        ] {
            let (path, server) = queued_server(Some(failure));
            let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
            let result = rpc.prompt(
                "owned-thread",
                "after interrupt",
                false,
                "after-interrupt",
                None,
            );
            drop(rpc);
            let calls = server.join().unwrap();
            let error = result
                .expect_err("lost/rejected wakeup must not claim success or replay admitted input");
            assert_ne!(error.kind(), io::ErrorKind::WouldBlock);
            assert!(
                error.to_string().contains("thread/queue/start"),
                "lost or rejected wakeup must name thread/queue/start: {error}"
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|v| v["method"] == "thread/queue/add")
                    .count(),
                1
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|v| v["method"] == "thread/queue/start")
                    .count(),
                1
            );
        }
    }

    fn idle_continuation_server() -> (std::path::PathBuf, std::thread::JoinHandle<Vec<Value>>) {
        let dir = crate::utils::test_temp::unique_temp_path("codex-idle-continuation");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut calls = Vec::new();
            while let Ok(frame) = ws.read() {
                let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                calls.push(value.clone());
                let Some(id) = value.get("id") else { continue };
                let result = match value["method"].as_str().unwrap() {
                    "initialize" => json!({"userAgent":"fixture"}),
                    "thread/read" => {
                        json!({"thread":{"id":"owned-thread","status":{"type":"idle"},"turns":[]}})
                    }
                    "thread/queue/add" => {
                        json!({"queuedSubmission":{"id":"continuation-queue","clientUserMessageId":value["params"]["clientUserMessageId"]}})
                    }
                    "thread/queue/start" => {
                        json!({"turn":{"id":"continuation-turn","status":"inProgress"}})
                    }
                    other => panic!("unexpected continuation operation {other}"),
                };
                ws.send(tungstenite::Message::Text(
                    json!({"id":id,"result":result}).to_string().into(),
                ))
                .unwrap();
                if value["method"] == "thread/queue/start" {
                    break;
                }
            }
            std::fs::remove_dir_all(dir).unwrap();
            calls
        });
        (path, handle)
    }

    #[test]
    fn codex_native_idle_background_continuation_starts_a_new_turn() {
        let (path, server) = idle_continuation_server();
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.prompt(
            "owned-thread",
            "continue after the background task",
            true,
            "continuation-input",
            Some("completed-old-turn"),
        )
        .unwrap();
        let calls = server.join().unwrap();
        assert!(calls
            .iter()
            .any(|call| call["method"] == "thread/queue/add"));
        assert!(!calls.iter().any(|call| call["method"] == "turn/steer"));
    }

    #[test]
    fn codex_native_developer_turn_interleaving_keeps_queue_input_independent() {
        let dir = crate::utils::test_temp::unique_temp_path("codex-developer-interleaving");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let mut developer_active = false;
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut calls = Vec::new();
            while let Ok(frame) = ws.read() {
                let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                calls.push(value.clone());
                let Some(id) = value.get("id") else { continue };
                let result = match value["method"].as_str().unwrap() {
                    "initialize" => json!({"userAgent":"fixture"}),
                    "thread/read" => {
                        json!({"thread":{"id":"owned-thread","status":{"type":"idle"},"turns":[]}})
                    }
                    "thread/queue/add" => {
                        assert!(
                            developer_active,
                            "developer Enter must precede Bus admission"
                        );
                        developer_active = false;
                        ws.send(tungstenite::Message::Text(json!({"method":"turn/completed","params":{"threadId":"owned-thread","turn":{"id":"developer-turn","status":"completed"}}}).to_string().into())).unwrap();
                        json!({"queuedSubmission":{"id":"independent-queue","clientUserMessageId":value["params"]["clientUserMessageId"]}})
                    }
                    "thread/queue/start" => {
                        assert!(!developer_active);
                        json!({"turn":{"id":"independent-turn","status":"inProgress"}})
                    }
                    other => panic!("queue input must not steer the developer turn: {other}"),
                };
                ws.send(tungstenite::Message::Text(
                    json!({"id":id,"result":result}).to_string().into(),
                ))
                .unwrap();
                if value["method"] == "thread/read" {
                    developer_active = true;
                    ws.send(tungstenite::Message::Text(json!({"method":"turn/started","params":{"threadId":"owned-thread","turn":{"id":"developer-turn","status":"inProgress"}}}).to_string().into())).unwrap();
                }
                if value["method"] == "thread/queue/start" {
                    break;
                }
            }
            std::fs::remove_dir_all(dir).unwrap();
            calls
        });
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.prompt(
            "owned-thread",
            "independent",
            false,
            "independent-input",
            None,
        )
        .unwrap();
        let calls = handle.join().unwrap();
        assert!(calls
            .iter()
            .any(|call| call["method"] == "thread/queue/add"));
        assert!(!calls.iter().any(|call| call["method"] == "turn/steer"));
    }

    #[test]
    fn codex_native_large_history_does_not_block_steer() {
        large_history_operation("steer");
    }

    #[test]
    fn codex_native_large_history_does_not_block_interrupt() {
        large_history_operation("interrupt");
    }

    fn large_history_operation(operation: &str) {
        {
            let dir = crate::utils::test_temp::unique_temp_path("codex-large-history");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("s");
            let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
            let handle = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                let mut calls = Vec::new();
                let mut interrupted = false;
                let mut enumerated = false;
                while let Ok(frame) = ws.read() {
                    let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                    calls.push(value.clone());
                    let Some(id) = value.get("id") else { continue };
                    let result = match value["method"].as_str().unwrap() {
                        "initialize" => json!({"userAgent":"fixture"}),
                        "thread/read"
                            if !value["params"]["includeTurns"].as_bool().unwrap_or(false) =>
                        {
                            json!({"thread":{"id":"owned-thread","status":if interrupted {json!({"type":"idle"})} else {json!({"type":"active","activeFlags":[]})}}})
                        }
                        "thread/read" => json!({"thread":{
                            "id":"owned-thread",
                            "status":{"type":"active","activeFlags":[]},
                            "turns":[{"id":"busy-turn","status":"inProgress","items":[{"type":"text","text":"x".repeat(9 * 1024 * 1024)}]}]
                        }}),
                        "thread/turns/list" => {
                            assert_eq!(value["params"]["itemsView"], "notLoaded");
                            assert_eq!(value["params"]["limit"], 1);
                            json!({"data":[{"id":"busy-turn","status":if interrupted {"interrupted"} else {"inProgress"},"items":[]}],"nextCursor":null})
                        }
                        "turn/steer" => json!({}),
                        "turn/interrupt" => {
                            interrupted = true;
                            json!({})
                        }
                        "thread/backgroundTerminals/list" => {
                            enumerated = true;
                            json!({"data":[],"nextCursor":null})
                        }
                        other => panic!("unexpected native operation {other}"),
                    };
                    ws.send(tungstenite::Message::Text(
                        json!({"id":id,"result":result}).to_string().into(),
                    ))
                    .unwrap();
                    if value["method"] == "turn/steer"
                        || (enumerated && value["method"] == "thread/turns/list")
                    {
                        break;
                    }
                }
                std::fs::remove_dir_all(dir).unwrap();
                calls
            });
            let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
            let result = if operation == "steer" {
                rpc.prompt(
                    "owned-thread",
                    "bounded steer",
                    true,
                    "large-history-steer",
                    Some("busy-turn"),
                )
                .map(|_| ())
            } else {
                rpc.interrupt("owned-thread").map(|_| ())
            };
            assert!(
                result.is_ok(),
                "{operation} must not hydrate full history: {result:?}"
            );
            let calls = handle.join().unwrap();
            assert!(calls.iter().all(|call| call["method"] != "turn/steer"
                || call["params"]["expectedTurnId"] == "busy-turn"));
        }
    }

    #[test]
    fn native_interrupt_ack_is_not_a_confirmed_stop() {
        let dir = crate::utils::test_temp::unique_temp_path("native-interrupt-confirmation");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let mut calls = Vec::new();
            while let Ok(frame) = ws.read() {
                let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                calls.push(value.clone());
                let Some(id) = value.get("id") else { continue };
                let result = match value["method"].as_str().unwrap() {
                    "initialize" => json!({"userAgent":"fixture"}),
                    "thread/read" => json!({"thread":{
                        "id":"owned-thread","status":{"type":"active","activeFlags":[]}
                    }}),
                    "thread/turns/list" => json!({"data":[{
                        "id":"busy-turn","status":"inProgress","items":[]
                    }],"nextCursor":null}),
                    "thread/backgroundTerminals/list" => json!({"data":[],"nextCursor":null}),
                    "turn/interrupt" => json!({}),
                    other => panic!("unexpected operation {other}"),
                };
                if ws
                    .send(tungstenite::Message::Text(
                        json!({"id":id,"result":result}).to_string().into(),
                    ))
                    .is_err()
                {
                    break;
                }
            }
            std::fs::remove_dir_all(dir).unwrap();
            calls
        });
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        let result = rpc.interrupt("owned-thread");
        drop(rpc);
        let calls = server.join().unwrap();
        assert!(calls.iter().any(|call| call["method"] == "turn/interrupt"));
        assert!(
            result.is_err(),
            "an acknowledgement while the turn remains active is not a confirmed stop"
        );
    }
    #[test]
    fn native_interrupt_requires_complete_owned_ended_metadata_and_empty_terminals() {
        for case in [
            "complete",
            "paged-empty",
            "live",
            "replacement",
            "late-replacement",
            "wrong-thread",
            "missing-turn",
            "invalid-status",
            "missing-data",
            "missing-cursor",
            "repeated-cursor",
            "too-many-pages",
            "unsupported",
            "transport",
        ] {
            let dir = crate::utils::test_temp::unique_temp_path("native-stop-controls");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("s");
            let listener = UnixListener::bind(&path).unwrap();
            let server = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                let mut interrupted = false;
                let mut page = 0;
                let mut calls = Vec::new();
                while let Ok(frame) = ws.read() {
                    let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                    calls.push(value.clone());
                    let Some(id) = value.get("id") else { continue };
                    let method = value["method"].as_str().unwrap();
                    let result = match method {
                        "initialize" => json!({}),
                        "thread/read" => json!({"thread":{
                            "id":if interrupted && case=="wrong-thread" {"other"} else {"owned-thread"},
                            "status":if interrupted {json!({"type":"idle"})} else {json!({"type":"active","activeFlags":[]})}
                        }}),
                        "thread/turns/list" => {
                            let mut result = json!({"data":[{
                                "id":if interrupted && (case=="replacement" || (case=="late-replacement" && page>0)) {"replacement-turn"} else {"busy-turn"},
                                "status":if !interrupted {"inProgress"} else if case=="invalid-status" {"unknown"} else {"interrupted"}
                            }],"nextCursor":null});
                            if interrupted && case == "missing-turn" {
                                result["data"] = json!([]);
                            }
                            result
                        }
                        "turn/interrupt" => {
                            interrupted = true;
                            json!({})
                        }
                        "thread/backgroundTerminals/list" => {
                            page += 1;
                            if case == "transport" {
                                break;
                            }
                            if case == "unsupported" {
                                ws.send(tungstenite::Message::Text(json!({"id":id,"error":{"code":-32601,"message":"unsupported"}}).to_string().into())).unwrap();
                                continue;
                            }
                            match case {
                                "live" => {
                                    json!({"data":[{"processId":"17","itemId":"old-background"}],"nextCursor":null})
                                }
                                "missing-data" => json!({"nextCursor":null}),
                                "missing-cursor" => json!({"data":[]}),
                                "repeated-cursor" => json!({"data":[],"nextCursor":"same"}),
                                "too-many-pages" => {
                                    json!({"data":[],"nextCursor":page.to_string()})
                                }
                                "paged-empty" if page == 1 => {
                                    json!({"data":[],"nextCursor":"page2"})
                                }
                                _ => json!({"data":[],"nextCursor":null}),
                            }
                        }
                        other => panic!("unexpected operation {other}"),
                    };
                    if ws
                        .send(tungstenite::Message::Text(
                            json!({"id":id,"result":result}).to_string().into(),
                        ))
                        .is_err()
                    {
                        break;
                    }
                }
                std::fs::remove_dir_all(dir).unwrap();
                calls
            });
            let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
            let result = rpc.interrupt("owned-thread");
            drop(rpc);
            let calls = server.join().unwrap();
            assert_eq!(
                result.is_ok(),
                matches!(case, "complete" | "paged-empty"),
                "{case}: {result:?}"
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|v| v["method"] == "turn/interrupt")
                    .count(),
                1
            );
            assert!(
                calls
                    .iter()
                    .filter(|v| v["method"] == "thread/backgroundTerminals/list")
                    .count()
                    <= 8
            );
            if case == "paged-empty" {
                assert!(calls.iter().any(|v| v["params"]["cursor"] == "page2"));
            }
            assert!(!calls.iter().any(|v| matches!(
                v["method"].as_str(),
                Some(
                    "thread/resume"
                        | "thread/backgroundTerminals/terminate"
                        | "thread/backgroundTerminals/clean"
                )
            )));
        }
    }
}
