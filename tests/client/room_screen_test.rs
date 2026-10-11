//! Built-binary room screen regression, including the Windows host-VT case.
#![cfg(any(unix, windows))]

#[path = "screen_support_test.rs"]
mod room_host_screen;
#[path = "room_screen_support_test.rs"]
mod room_screen_client;

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
