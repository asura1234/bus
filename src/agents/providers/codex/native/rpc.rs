//! Local app-server RPC. Connection failure and an uncertain mutation differ.
use serde_json::{json, Value};
use std::{io, os::unix::net::UnixStream, path::Path, time::Instant};
use tungstenite::WebSocket;

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
        self.timeout()?;
        // From here a transport error must never cause fallback or replay.
        self.socket
            .send(tungstenite::Message::Text(
                json!({"id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .map_err(io::Error::other)?;
        loop {
            self.timeout()?;
            let frame = self.socket.read().map_err(io::Error::other)?;
            let tungstenite::Message::Text(text) = frame else {
                if frame.is_close() {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Codex app-server disconnected",
                    ));
                }
                continue;
            };
            let value: Value = serde_json::from_str(&text)?;
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
                    "thread/read",
                    json!({"threadId":thread,"includeTurns":true}),
                )?;
                if history["thread"]["id"] != thread {
                    return Err(io::Error::other("Codex native thread identity mismatch"));
                }
                let turn = history["thread"]["turns"]
                    .as_array()
                    .and_then(|turns| turns.last())
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
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "Codex native preflight refused before input",
            )
        })?;
        if steer && (expected_turn.is_none() || active.as_deref() != expected_turn) {
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
        let mut input: Vec<Value> = images
            .into_iter()
            .map(|image| json!({"type":"localImage","path":serde_json::from_str::<String>(image).unwrap_or_else(|_|image.to_owned())}))
            .collect();
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
        let admitted = self.call(method, params)?;
        if method == "thread/queue/add" {
            let queued = admitted["queuedSubmission"]["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| io::Error::other("Codex admitted queue receipt has no identity"))?;
            // Interrupt pauses Codex's queue. Wake exactly the receipted entry;
            // turn/start could accidentally steer a racing developer turn.
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

    pub(super) fn interrupt(&mut self, thread: &str) -> io::Result<()> {
        if let Some(turn) = self.active_turn(thread, true)? {
            self.call("turn/interrupt", json!({"threadId":thread,"turnId":turn}))?;
        }
        Ok(())
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
                        json!({"thread":{"id":thread, "status":if active {json!({"type":"active","activeFlags":[]})} else {json!({"type":"idle"})}, "turns":if active {json!([{"id":"busy-turn","status":"inProgress","items":[]}])} else {json!([])}}})
                    }
                    "thread/turns/list" => {
                        json!({"data":[{"id":"busy-turn","status":"inProgress","items":[]}],"nextCursor":null})
                    }
                    "turn/interrupt" => json!({}),
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
                if method.starts_with("turn/") || method == "thread/queue/start" {
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
    fn codex_native_idle_admission_cannot_join_a_racing_developer_turn() {
        let (path, handle) = server(false, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.prompt("owned-thread", "independent", false, "racing-idle", None)
            .unwrap();
        let calls = handle.join().unwrap();
        // turn/start is start-or-steer in Codex; a native queue entry stays
        // independent even if the developer presses Enter after thread/read.
        assert_eq!(calls.last().unwrap()["method"], "thread/queue/start");
        assert!(!calls.iter().any(|v| v["method"] == "turn/start"));
    }

    #[test]
    fn codex_native_steer_does_not_require_the_unimplemented_turns_list_api() {
        let (path, handle) = server(true, false);
        let mut rpc = Rpc::connect(&path, Instant::now() + Duration::from_secs(3)).unwrap();
        rpc.prompt("owned-thread", "steer", true, "busy-id", Some("busy-turn"))
            .unwrap();
        let calls = handle.join().unwrap();
        assert!(!calls
            .iter()
            .any(|call| call["method"] == "thread/turns/list"));
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
        assert!(rpc
            .prompt("owned-thread", "once", false, "bus-msg-9", None)
            .is_err());
        let calls = handle.join().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["method"] == "thread/queue/add")
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
        rpc.interrupt("owned-thread").unwrap();
        let calls = handle.join().unwrap();
        assert_eq!(calls.last().unwrap()["method"], "turn/interrupt");
        assert_eq!(
            calls.last().unwrap()["params"],
            json!({"threadId":"owned-thread","turnId":"busy-turn"})
        );
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
}
