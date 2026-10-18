use super::*;
use crate::messaging::model::{AgentId, RoomId};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

struct Fixture {
    root: PathBuf,
    lease: std::fs::File,
}

impl Fixture {
    fn new() -> Self {
        let lease = process::fixture_lease();
        let root = crate::utils::test_temp::unique_temp_path("feishu-self-send");
        std::fs::create_dir_all(&root).unwrap();
        let program = root.join("lark-cli");
        std::fs::write(&program, include_str!("lark_fixture.txt")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, lease }
    }

    fn client(&self) -> Client {
        Client::at(self.root.join("lark-cli"), Duration::from_secs(10))
    }

    fn mode(&self, mode: &str) {
        std::fs::write(self.root.join("mode"), mode).unwrap();
    }

    fn calls(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = self.lease.unlock();
    }
}

fn message() -> Message {
    Message::new(
        "office-mac",
        "session-123",
        "notice:1".into(),
        &AttentionOrigin {
            room_id: RoomId(4),
            room_name: "release".into(),
            agent_id: AgentId(5),
            agent_name: "orch".into(),
        },
        "[needs_developer_attention] Choose a target.\nLiteral $(not-a-command)",
    )
}

#[test]
fn feishu_send_is_self_only_and_always_attributed_to_bus_with_all_context() {
    let fixture = Fixture::new();
    let user = fixture
        .client()
        .authenticate(Cancellation::default())
        .unwrap();
    user.send(&message()).unwrap();
    let calls = fixture.calls();
    assert_eq!(calls.len(), 3, "enable auth, send-time auth, one send");
    let send = calls.last().unwrap();
    let args = send["args"].as_array().unwrap();
    let field = |name: &str| {
        args[args.iter().position(|value| value == name).unwrap() + 1]
            .as_str()
            .unwrap()
    };
    assert_eq!(field("--user-id"), send["self"]);
    assert_eq!(field("--as"), "user");
    assert_eq!(field("--text"), "-");
    assert!(field("--idempotency-key").len() <= 50);
    assert!(!args.iter().any(|value| value == "--chat-id"));
    let body = send["body"].as_str().unwrap();
    assert!(body.starts_with("[Bus]"));
    for text in [
        "office-mac",
        "session-123",
        "release (4)",
        "orch (5)",
        "Choose a target.",
        "Literal $(not-a-command)",
    ] {
        assert!(body.contains(text), "{body}");
    }
}

#[test]
fn feishu_send_reverification_refuses_changed_or_missing_user_before_any_send() {
    for mode in ["changed", "missing"] {
        let fixture = Fixture::new();
        let user = fixture
            .client()
            .authenticate(Cancellation::default())
            .unwrap();
        fixture.mode(mode);
        let error = user.send(&message()).unwrap_err();
        assert!(error.disable, "{error:?}");
        assert!(fixture.calls().iter().all(|call| call["args"][0] == "auth"));
    }
}

#[test]
fn feishu_send_failure_invokes_once_and_does_not_expose_cli_payload() {
    let fixture = Fixture::new();
    let user = fixture
        .client()
        .authenticate(Cancellation::default())
        .unwrap();
    fixture.mode("send-error");
    let error = user.send(&message()).unwrap_err();
    assert!(!error.message.contains("DO_NOT_LEAK"));
    let calls = fixture.calls();
    let sends: Vec<_> = calls
        .iter()
        .filter(|call| call["args"][0] == "im")
        .collect();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0]["args"][5], sends[0]["self"]);
}
