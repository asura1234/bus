//! Source-preserving direct and live-viewer routes for machine commands.
use super::{control, run_with, Request, Response};
use std::{
    io::{self, Write},
    path::Path,
};

/// The normal grammar over SSH, with no mutation retries or remote ID aliasing.
pub(in crate::cli) fn run_remote(target: &str, args: &[String]) -> io::Result<()> {
    let result = run_remote_with(args, &mut io::stdout().lock(), remote_sender, |request| {
        crate::messaging::machines::request(target, request)
    });
    if result.is_err() {
        std::process::exit(1);
    }
    result
}

/// A machine without a saved target can use a viewing Bus's already-live route.
pub(in crate::cli) fn run_remote_forwarded(machine: &str, args: &[String]) -> io::Result<()> {
    let root = local_bus_root();
    let result = run_remote_with(args, &mut io::stdout().lock(), remote_sender, |request| {
        let Some(root) = root.as_deref() else {
            return Ok(unavailable_route(
                &request.id,
                "No local Bus or live viewer route is available",
            ));
        };
        forward_request_with(root, machine, request, control::request)
    });
    if result.is_err() {
        std::process::exit(1);
    }
    result
}

fn forward_request_with(
    root: &Path,
    machine: &str,
    request: &Request,
    mut send: impl FnMut(&Path, &Request) -> Result<Response, String>,
) -> Result<Response, String> {
    let mut forwarded = send(
        root,
        &Request {
            id: format!("{}-forward", request.id),
            method: "machine.forward".into(),
            params: serde_json::json!({"machine":machine,"request":request}),
        },
    )?;
    if !forwarded.ok {
        // A broker admission failure is an authoritative non-delivery receipt.
        // Keep it distinct from losing the broker's acknowledgement after dispatch.
        forwarded.id.clone_from(&request.id);
        if forwarded.result.is_null()
            && forwarded.error.as_ref().is_some_and(|error| {
                matches!(
                    error.code.as_str(),
                    "machine_unavailable" | "ambiguous_machine" | "route_busy" | "invalid_request"
                )
            })
        {
            forwarded.result = serde_json::json!({"applied":false});
        }
        return Ok(forwarded);
    }
    let response: Response = serde_json::from_value(forwarded.result)
        .map_err(|_| "Live viewer returned an invalid destination response")?;
    if response.id != request.id || response.ok == response.error.is_some() {
        return Err("Live viewer returned a mismatched destination response".into());
    }
    Ok(response)
}

fn unavailable_route(id: &str, message: &str) -> Response {
    let mut response = Response::failure(id, "machine_unavailable", message);
    response.result = serde_json::json!({"applied":false});
    response
}

fn run_remote_with(
    args: &[String],
    output: &mut impl Write,
    mut sender: impl FnMut(&Request) -> Result<serde_json::Value, String>,
    mut transport: impl FnMut(&Request) -> Result<Response, String>,
) -> io::Result<()> {
    let mut sent = std::collections::HashSet::new();
    run_with(args, output, |request, _timeout| {
        let mut request = request.clone();
        // Busy handling in the ordinary CLI may repeat an unsent local command.
        // Across SSH every mutation has exactly one attempt, even after rejection.
        let read_only = matches!(
            request.method.as_str(),
            "state"
                | "message.status"
                | "room.history"
                | "agent.list"
                | "agent.read"
                | "agent.observe"
                | "sounds"
                | "settings"
        );
        if !read_only && !sent.insert(request.id.clone()) {
            return Err("Remote command was already attempted and was not retried".into());
        }
        if request.method == "message.send" {
            if request.params.get("reply_to").is_some() {
                return Err("Cross-machine --reply-to cannot settle another machine's request; send a new message with --room and --to instead".into());
            }
            let external_author = sender(&request)?;
            let params = request
                .params
                .as_object_mut()
                .ok_or("Invalid send parameters")?;
            params.remove("as");
            params.remove("caller");
            params.insert("external_author".into(), external_author);
        } else if request.method == "message.status" {
            // A source-machine agent ID is never a destination-machine actor.
            if let Some(params) = request.params.as_object_mut() {
                params.remove("as");
            }
        } else if request.method == "message.seen" {
            return Err(
                "Remote replies are not acknowledged using a source-machine agent ID".into(),
            );
        }
        transport(&request)
    })
}

fn local_bus_root() -> Option<std::path::PathBuf> {
    crate::utils::env::bus_data_dir().or_else(|| {
        crate::messaging::storage::sessions::default_base_dir()
            .ok()
            .and_then(|base| {
                crate::messaging::storage::sessions::LocalSessionRegistry::new(base)
                    .last()
                    .ok()
            })
            .map(|session| session.root)
    })
}

fn remote_sender(request: &Request) -> Result<serde_json::Value, String> {
    let root = local_bus_root();
    let agent_context = [
        "BUS_LAUNCH_ID",
        "BUS_CALLBACK_DIR",
        "HERDR_AGENT",
        "HERDR_PANE_ID",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some());
    remote_sender_from(request, root.as_deref(), agent_context, control::request)
}

fn remote_sender_from(
    request: &Request,
    root: Option<&Path>,
    agent_context: bool,
    mut send: impl FnMut(&Path, &Request) -> Result<Response, String>,
) -> Result<serde_json::Value, String> {
    if request.params.get("as").is_none()
        && request.params.get("caller").is_none()
        && !agent_context
    {
        return Ok(serde_json::Value::Null);
    }
    let root = root.ok_or("Source Bus unavailable; cannot resolve the cross-machine sender")?;
    let mut params = serde_json::json!({});
    for field in ["as", "caller"] {
        if let Some(value) = request.params.get(field) {
            params[field] = value.clone();
        }
    }
    let source = send(
        root,
        &Request {
            id: format!("{}-source", request.id),
            method: "message.sender".into(),
            params,
        },
    )?;
    if !source.ok {
        return Err(source.error.map_or_else(
            || "Cannot resolve source sender".into(),
            |error| error.message,
        ));
    }
    source
        .result
        .get("external_author")
        .cloned()
        .ok_or("Source Bus did not return sender identity".into())
}

#[cfg(test)]
mod remote_tests {
    use super::*;

    #[test]
    fn remote_source_agent_without_as_keeps_original_caller_identity() {
        let caller = serde_json::json!({"launch_id":"source-launch", "pane_id":"source-pane", "agent_context":true, "bus_context":true, "process":{"pid":123,"birth":456}});
        let request = Request {
            id: "send-1".into(),
            method: "message.send".into(),
            params: serde_json::json!({"caller":caller,"room":"destination-room","to":["worker"],"text":"work"}),
        };
        let expected = serde_json::json!({"machine":"source","name":"author","agent_id":17});
        let author = remote_sender_from(
            &request,
            Some(Path::new("/source-bus")),
            false,
            |root, source| {
                assert_eq!(root, Path::new("/source-bus"));
                assert_eq!(source.method, "message.sender");
                assert_eq!(source.params, serde_json::json!({"caller":caller}));
                Ok(Response::success(
                    &source.id,
                    serde_json::json!({"external_author":expected}),
                ))
            },
        )
        .unwrap();
        assert_eq!(author, expected);
    }

    #[test]
    fn remote_source_agent_context_uses_source_capture_when_no_caller_was_prepared() {
        let request = Request {
            id: "source-capture".into(),
            method: "message.send".into(),
            params: serde_json::json!({"room":"remote"}),
        };
        let expected = serde_json::json!({"machine":"source","name":"inferred","agent_id":23});
        let author = remote_sender_from(
            &request,
            Some(Path::new("/source-bus")),
            true,
            |_, source| {
                // control::request attaches its local process capture to message.sender.
                assert_eq!(source.method, "message.sender");
                assert_eq!(source.params, serde_json::json!({}));
                Ok(Response::success(
                    &source.id,
                    serde_json::json!({"external_author":expected}),
                ))
            },
        )
        .unwrap();
        assert_eq!(author, expected);
    }

    #[test]
    fn remote_forward_without_saved_target_preserves_source_and_destination_receipt() {
        let args = [
            "send",
            "--room",
            "source-work",
            "--as",
            "source-agent",
            "--to",
            "destination-agent",
            "--text",
            "task",
        ]
        .map(str::to_owned);
        let provenance = serde_json::json!({"machine":"B","machine_id":"machine-b","name":"source-agent","agent_id":17,"room_id":2,"room_name":"source-work","home_is_master":false});
        let mut output = Vec::new();
        let mut calls = 0;
        run_remote_with(
            &args,
            &mut output,
            |_| Ok(provenance.clone()),
            |request| {
                forward_request_with(Path::new("/machine-b"), "C", request, |root, forwarded| {
                    calls += 1;
                    assert_eq!(root, Path::new("/machine-b"));
                    assert_eq!(forwarded.method, "machine.forward");
                    assert_eq!(forwarded.params["machine"], "C");
                    let inner: Request =
                        serde_json::from_value(forwarded.params["request"].clone()).unwrap();
                    assert_eq!(inner.params["external_author"], provenance);
                    assert!(inner.params.get("as").is_none());
                    assert_eq!(inner.params["to"], serde_json::json!(["destination-agent"]));
                    let destination = Response::success(
                        &inner.id,
                        serde_json::json!({"message_id":7,"stage":"queued"}),
                    );
                    Ok(Response::success(
                        &forwarded.id,
                        serde_json::to_value(destination).unwrap(),
                    ))
                })
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
        let result: Response = serde_json::from_slice(&output).unwrap();
        assert!(result.ok);
        assert_eq!(result.result["message_id"], 7);
    }

    #[test]
    fn remote_forward_without_a_live_route_fails_once() {
        let args = ["send", "--room", "1", "--to", "worker", "--text", "task"].map(str::to_owned);
        let mut output = Vec::new();
        let mut calls = 0;
        let result = run_remote_with(
            &args,
            &mut output,
            |_| Ok(serde_json::Value::Null),
            |request| {
                forward_request_with(Path::new("/machine-b"), "C", request, |_, forwarded| {
                    calls += 1;
                    Ok(Response::failure(
                        &forwarded.id,
                        "machine_unavailable",
                        "No live viewer route to C",
                    ))
                })
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
        let result: Response = serde_json::from_slice(&output).unwrap();
        assert!(!result.ok);
        assert_eq!(result.result["applied"], false);
        let error = result.error.unwrap();
        assert_eq!(error.code, "machine_unavailable");
        assert!(error.message.contains("No live viewer route to C"));
    }

    #[test]
    fn remote_forward_lost_ack_is_not_submitted_again() {
        let args = ["send", "--room", "1", "--to", "worker", "--text", "once"].map(str::to_owned);
        let mut output = Vec::new();
        let mut calls = 0;
        assert!(run_remote_with(
            &args,
            &mut output,
            |_| Ok(serde_json::Value::Null),
            |request| {
                forward_request_with(Path::new("/machine-b"), "C", request, |_, _| {
                    calls += 1;
                    Err("Route closed after submission; outcome unknown".into())
                })
            }
        )
        .is_err());
        assert_eq!(calls, 1);
        let result: Response = serde_json::from_slice(&output).unwrap();
        assert!(!result.ok);
        assert!(result.error.unwrap().message.contains("outcome unknown"));
    }
}
