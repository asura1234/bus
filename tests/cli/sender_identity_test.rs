//! Exercise the installed CLI path with a private control endpoint, never a live Bus.
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

enum Caller {
    Agent,
    AgentChild,
    Developer,
}

fn send(args: &[&str], caller: Caller) -> (std::process::Output, serde_json::Value) {
    let root = crate::fixture_dirs::unique_temp_path("bus-sender");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(root.join("control.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (tx, rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut line = String::new();
                    BufReader::new(stream.try_clone().unwrap())
                        .read_line(&mut line)
                        .unwrap();
                    let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let reply = serde_json::json!({"id":request["id"],"ok":true,"result":{"message_id":1,"request_ids":[2],"stage":"queued"},"error":null});
                    writeln!(stream, "{reply}").unwrap();
                    tx.send(request).unwrap();
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        }
    });
    let mut command = match caller {
        Caller::Agent => Command::new(env!("CARGO_BIN_EXE_bus")),
        Caller::AgentChild => {
            let mut command = Command::new("/bin/sh");
            // Keep the pane shell alive while its child drops inherited hints.
            command.args([
                "-c",
                "env -u BUS_LAUNCH_ID -u HERDR_PANE_ID -u HERDR_ENV \"$@\"; exit $?",
                "sh",
                env!("CARGO_BIN_EXE_bus"),
            ]);
            command
        }
        Caller::Developer => Command::new(env!("CARGO_BIN_EXE_bus")),
    };
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("BUS_") || key.to_string_lossy().starts_with("HERDR_")
        {
            command.env_remove(key);
        }
    }
    command.args(args).env("BUS_DATA_DIR", &root);
    if !matches!(caller, Caller::Developer) {
        command
            .env("BUS_LAUNCH_ID", "owner-launch")
            .env("HERDR_PANE_ID", "w1:p2")
            .env("HERDR_ENV", "1");
    }
    let output = command.output().unwrap();
    server.join().unwrap();
    let request = rx.try_recv().unwrap_or(serde_json::Value::Null);
    std::fs::remove_dir_all(root).unwrap();
    (output, request)
}

#[test]
fn sender_identity_cli_passes_inherited_agent_context_for_ordinary_and_reply_sends() {
    for args in [
        vec!["send", "--room", "7", "--to", "peer", "--text", "window"],
        vec!["send", "--reply-to", "123", "--text", "done"],
    ] {
        let (output, request) = send(&args, Caller::Agent);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(request["params"]["caller"]["launch_id"], "owner-launch");
        assert_eq!(request["params"]["caller"]["pane_id"], "w1:p2");
        assert_eq!(request["params"]["caller"]["agent_context"], true);
        assert!(request["params"].get("as").is_none());
    }
}

#[test]
fn sender_identity_cli_explicit_author_remains_explicit() {
    let (output, request) = send(
        &[
            "send", "--room", "7", "--to", "peer", "--as", "owner", "--text", "window",
        ],
        Caller::Agent,
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(request["params"]["as"], "owner");
    assert!(request["params"].get("caller").is_none());
}

#[test]
fn sender_identity_cli_carries_process_identity_when_parent_environment_is_hidden() {
    let (output, request) = send(
        &["send", "--room", "7", "--to", "peer", "--text", "window"],
        Caller::AgentChild,
    );
    assert!(output.status.success(), "{output:?}");
    assert!(request["params"]["caller"]["launch_id"].is_null());
    assert!(request["params"]["caller"]["process"]["pid"]
        .as_u64()
        .is_some());
    assert!(request["params"]["caller"]["process"]["birth"]
        .as_u64()
        .is_some());
}

#[test]
fn sender_identity_cli_external_shell_carries_process_identity() {
    let (output, request) = send(
        &["send", "--room", "7", "--to", "peer", "--text", "window"],
        Caller::Developer,
    );
    assert!(output.status.success(), "{output:?}");
    assert!(request["params"]["caller"]["process"]["pid"]
        .as_u64()
        .is_some());
    assert_eq!(request["params"]["caller"]["agent_context"], false);
}
