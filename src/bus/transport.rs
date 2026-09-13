//! Bounded direct JSON API transport, deliberately independent of the TUI endpoint.
use crate::api::{
    client::{ApiClient, ApiClientError, ConnectionTarget},
    schema::{Method, Request, ResponseResult},
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub(crate) struct TransportError {
    pub(crate) code: Option<String>,
    pub(crate) message: String,
    pub(crate) definitely_rejected: bool,
}

pub(crate) trait Transport: Send {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError>;
}

pub(crate) struct HerdrTransport {
    client: ApiClient,
    next_id: u64,
}

impl HerdrTransport {
    pub(crate) fn new(target: ConnectionTarget) -> Self {
        Self {
            client: ApiClient::for_target(target),
            next_id: 0,
        }
    }
}

impl Transport for HerdrTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.next_id += 1;
        let request = Request {
            id: format!("bus:{}:{}", std::process::id(), self.next_id),
            method,
        };
        let shell_deadline = Instant::now() + Duration::from_secs(5);
        let started = Instant::now();
        let method_name = crate::api::api_method_name(&request.method);
        let _span =
            tracing::debug_span!("bus.api", api_request_id = %request.id, method = method_name)
                .entered();
        tracing::debug!(event = "bus.api.start", api_request_id = %request.id,
            method = method_name, "Bus native API call");
        loop {
            let response = self
                .client
                .request_value_with_timeout(&request, Duration::from_secs(3))
                .and_then(crate::api::client::parse_response_value);
            // TabCreate can return while the login shell is running startup
            // children. This precise rejection occurs before any PTY input or
            // managed launch mutation. Never retry a timeout, input failure,
            // readiness failure after launch, or any prompt submission.
            if retry_shell_start(&request.method, &response, Instant::now() < shell_deadline) {
                tracing::debug!(
                    event = "bus.api.retry",
                    reason = "shell_starting",
                    "Launch rejected before input; retrying"
                );
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            let result = response
                .map(|response| response.result)
                .map_err(classify_error);
            tracing::debug!(event = "bus.api.result", api_request_id = %request.id, method = method_name,
                outcome = if result.is_ok() { "ok" } else { "error" },
                error_code = ?result.as_ref().err().and_then(|e| e.code.as_deref()),
                definitely_rejected = result.as_ref().err().is_some_and(|e| e.definitely_rejected),
                elapsed_ms = started.elapsed().as_millis() as u64, "Bus native API result");
            return result;
        }
    }
}

fn retry_shell_start(
    method: &Method,
    response: &Result<crate::api::schema::SuccessResponse, ApiClientError>,
    before_deadline: bool,
) -> bool {
    before_deadline
        && matches!(method, Method::AgentStart(_))
        && matches!(response, Err(ApiClientError::ErrorResponse(response)) if response.error.code == "agent_pane_busy")
}

fn classify_error(error: ApiClientError) -> TransportError {
    // These codes originate before enqueueing PTY input. Every other failure,
    // including an IO error after partial send, leaves submission ownership intact.
    let definitely_rejected = matches!(&error, ApiClientError::ErrorResponse(response) if matches!(response.error.code.as_str(),
        "agent_not_idle" | "agent_identity_changed" | "agent_not_ready" | "agent_blocked" | "agent_not_found" | "empty_agent_prompt" | "unknown_method" | "invalid_request"));
    TransportError {
        code: match &error {
            ApiClientError::ErrorResponse(response) => Some(response.error.code.clone()),
            _ => None,
        },
        message: error.to_string(),
        definitely_rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_retry_never_repeats_uncertain_start_or_prompt_or_exceeds_deadline() {
        let start = Method::AgentStart(crate::api::schema::AgentStartParams {
            name: "live".into(),
            kind: "claude".into(),
            pane_id: "w1:p2".into(),
            args: vec![],
            timeout_ms: None,
        });
        for code in [
            "agent_pane_busy",
            "timeout",
            "agent_start_input_failed",
            "agent_not_ready",
        ] {
            let error = Err(ApiClientError::ErrorResponse(
                crate::api::schema::ErrorResponse {
                    id: "r".into(),
                    error: crate::api::schema::ErrorBody {
                        code: code.into(),
                        message: code.into(),
                    },
                },
            ));
            assert_eq!(
                retry_shell_start(&start, &error, true),
                code == "agent_pane_busy"
            );
            assert!(!retry_shell_start(&start, &error, false));
            assert!(!retry_shell_start(
                &Method::AgentList(crate::api::schema::EmptyParams {}),
                &error,
                true
            ));
        }
        assert!(!retry_shell_start(
            &start,
            &Err(ApiClientError::Io(std::io::Error::other("lost response"))),
            true
        ));
    }

    #[test]
    fn newly_created_shell_rejection_is_retried_before_launch_only() {
        use interprocess::local_socket::traits::Listener;
        use std::io::{BufRead, Write};
        let dir = std::env::temp_dir().join(format!("bus-start-{}", super::super::io::now_ns()));
        super::super::io::private_dir(&dir).unwrap();
        let path = dir.join("api.sock");
        let listener = crate::ipc::bind_local_listener(&path).unwrap();
        let server = std::thread::spawn(move || {
            for code in [Some("agent_pane_busy"), None] {
                let mut stream = listener.accept().unwrap();
                let mut line = String::new();
                std::io::BufReader::new(&mut stream)
                    .read_line(&mut line)
                    .unwrap();
                let value: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(value["method"], "agent.start");
                assert_eq!(value["params"]["pane_id"], "w1:p2");
                let response = match code {
                    Some(code) => {
                        serde_json::json!({"id":value["id"],"error":{"code":code,"message":"shell still starting"}})
                    }
                    None => serde_json::json!({"id":value["id"],"result":{"type":"ok"}}),
                };
                writeln!(stream, "{response}").unwrap();
            }
        });
        let mut client = HerdrTransport::new(ConnectionTarget::SocketPath(path));
        let outcome = client.request(Method::AgentStart(crate::api::schema::AgentStartParams {
            name: "live".into(),
            kind: "claude".into(),
            pane_id: "w1:p2".into(),
            args: vec![],
            timeout_ms: None,
        }));
        assert!(outcome.is_ok(), "{outcome:?}");
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn network_and_post_enqueue_errors_are_always_uncertain() {
        assert!(
            !classify_error(ApiClientError::Io(std::io::Error::other(
                "connection lost after write"
            )))
            .definitely_rejected
        );
        for (code, rejected) in [
            ("agent_not_idle", true),
            ("invalid_request", true),
            ("unknown_method", true),
            ("timeout", false),
            ("agent_prompt_failed", false),
        ] {
            let error = crate::api::schema::ErrorResponse {
                id: "r".into(),
                error: crate::api::schema::ErrorBody {
                    code: code.into(),
                    message: code.into(),
                },
            };
            let classified = classify_error(ApiClientError::ErrorResponse(error));
            assert_eq!(classified.definitely_rejected, rejected);
            assert_eq!(classified.code.as_deref(), Some(code));
        }
    }

    #[test]
    fn actual_json_socket_uses_guarded_method_and_old_server_rejects_it() {
        use interprocess::local_socket::traits::Listener;
        use std::io::{BufRead, Write};
        let dir = std::env::temp_dir().join(format!("bus-socket-{}", super::super::io::now_ns()));
        super::super::io::private_dir(&dir).unwrap();
        let path = dir.join("api.sock");
        let listener = crate::ipc::bind_local_listener(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&mut stream)
                .read_line(&mut line)
                .unwrap();
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(value["method"], "agent.prompt_if_idle");
            assert_eq!(value["params"]["expected_session_id"], "s");
            writeln!(stream,"{{\"id\":\"bus:1\",\"error\":{{\"code\":\"invalid_request\",\"message\":\"unknown method\"}}}}").unwrap();
        });
        let mut client = HerdrTransport::new(ConnectionTarget::SocketPath(path));
        let outcome = client.request(Method::AgentPromptIfIdle(
            crate::api::schema::AgentPromptIfIdleParams {
                target: "t".into(),
                text: "literal $HOME @x".into(),
                expected_terminal_id: "t".into(),
                expected_pane_id: "p".into(),
                expected_agent: "codex".into(),
                expected_session_id: "s".into(),
            },
        ));
        assert!(outcome.unwrap_err().definitely_rejected);
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
