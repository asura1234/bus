//! Built-binary room screen regression, including the Windows host-VT case.
#![cfg(any(unix, windows))]

#[path = "../../src/utils/tests/live_bus_env_test.rs"]
mod live_bus_env;
#[path = "screen_support_test.rs"]
mod room_host_screen;
#[path = "room_screen_support_test.rs"]
mod room_screen_client;
#[path = "../support/stop_test.rs"]
mod stop_support;

/// Observe the actual client's ANSI in a real host VT, outside the client component.
#[test]
fn a_screen_switch_repaints_stale_cells_in_the_right_margin() {
    let mut client = room_screen_client::RoomClient::spawn(room_screen_client::unique_test_dir());
    client.control("room.create", serde_json::json!({"name":"margin-first"}));
    client.control("room.create", serde_json::json!({"name":"margin-other"}));
    client.control("room.focus", serde_json::json!({"room":"margin-first"}));
    client.observe_room("margin-first");
    assert!(
        client.screen.margin().trim().is_empty(),
        "Bus never draws in the margin"
    );

    client.screen.write(b"\x1b[5;100Hr\x1b[20;100Hd");
    client.control(
        "room.notes",
        serde_json::json!({"room":"margin-first", "text":"changed-margin-notes"}),
    );
    client.observe("changed-margin-notes");
    assert_eq!(
        client.screen.margin().replace(' ', ""),
        "rd",
        "a diff frame keeps them"
    );

    client.control("room.focus", serde_json::json!({"room":"margin-other"}));
    client.observe_room("margin-other");
    assert!(
        client.screen.margin().trim().is_empty(),
        "{:?}",
        client.screen.margin()
    );
}

/// Every session answers the agent tier; dev tools answer only when the
/// session itself was started with --dev, whatever flag the command passes.
#[test]
fn a_session_without_dev_answers_agent_commands_and_refuses_dev_tools() {
    let client =
        room_screen_client::RoomClient::spawn_without_dev(room_screen_client::unique_test_dir());
    let response = |output: &std::process::Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)))
    };
    let state = client.cli(&["state"]);
    assert!(
        state.status.success(),
        "{}",
        String::from_utf8_lossy(&state.stderr)
    );
    let state = response(&state);
    assert!(state["result"]["agents"].is_array(), "{state}");
    client.control("room.create", serde_json::json!({"name":"plain"}));

    for args in [
        &["room", "focus", "plain"][..],
        &["--dev", "diagnostics"][..],
    ] {
        let refused = client.cli(args);
        assert!(!refused.status.success(), "{args:?}");
        assert_eq!(
            response(&refused)["error"]["code"],
            "dev_tools_disabled",
            "{args:?}"
        );
    }
}

/// Run the no-provider help examples against a real, private control server.
#[test]
fn help_examples_manage_a_scratch_room_without_provider_processes() {
    let client =
        room_screen_client::RoomClient::spawn_without_dev(room_screen_client::unique_test_dir());
    let run = |args: &[&str]| -> serde_json::Value {
        let output = client.cli(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], true, "{args:?}: {response}");
        response["result"].clone()
    };

    let created = run(&["room", "create", "feature-work"]);
    let id = created["room_id"].as_u64().unwrap();
    let replay = run(&[
        "room",
        "create",
        "release-check",
        "--request-id",
        "release-check-room-v1",
    ]);
    assert_eq!(
        replay,
        run(&[
            "room",
            "create",
            "release-check",
            "--request-id",
            "release-check-room-v1"
        ])
    );
    run(&["room", "notes", "feature-work", "--text", "Review pending"]);
    let state = run(&["state"]);
    let room = state["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|room| room["id"] == id)
        .unwrap();
    assert_eq!(room["notes"], "Review pending");
    assert!(state["build"]["binary"].is_string());
    assert!(run(&["agent", "list", "--room", "feature-work"])["agents"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(run(&["history", "--room", "feature-work"])["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    let page = run(&["history", "--room", "feature-work", "--limit", "100"]);
    assert_eq!(page["has_more"], false);
    assert!(page["next_before"].is_null());
    assert!(run(&["settings"])["settings"]["developer_sound"]["enabled"].is_boolean());
    run(&["room", "rename", "feature-work", "implementation"]);

    // Missing confirmation and protected MASTER are errors, not successful mutations.
    for args in [
        &["room", "delete", "implementation"][..],
        &["room", "notes", "master", "--text", "Review pending"],
    ] {
        let output = client.cli(args);
        assert!(!output.status.success());
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], false);
        assert!(response["error"]["code"].is_string());
    }
    run(&["room", "delete", "implementation", "--confirm"]);
    assert!(run(&["state"])["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .all(|room| room["id"] != id));
}
