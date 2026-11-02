//! Built-binary room screen regression, including the Windows host-VT case.
#![cfg(any(unix, windows))]

#[path = "../support/fixture_dir_test.rs"]
mod fixture_dirs;

#[path = "../../src/utils/tests/live_bus_env_test.rs"]
mod live_bus_env;
#[path = "screen_support_test.rs"]
mod room_host_screen;
#[path = "room_screen_support_test.rs"]
mod room_screen_client;
#[path = "../support/stop_test.rs"]
mod stop_support;

/// Fixtures allocated during the same clock tick must keep independent sessions.
#[test]
fn same_clock_fixture_sessions_survive_sibling_startup_and_teardown() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn panic_text(error: Box<dyn std::any::Any + Send>) -> String {
        error
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| error.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_else(|| "non-string panic".to_owned())
    }

    let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    let first_base = room_screen_client::unique_test_dir_at(timestamp);
    let second_base = room_screen_client::unique_test_dir_at(timestamp);
    let first = room_screen_client::RoomClient::spawn_without_dev(first_base.clone());
    let second = catch_unwind(AssertUnwindSafe(|| {
        room_screen_client::RoomClient::spawn_without_dev(second_base.clone())
    }))
    .map_err(panic_text);
    let create_room = |client: &room_screen_client::RoomClient, name| {
        catch_unwind(AssertUnwindSafe(|| {
            let output = client.cli(&["room", "create", name]);
            assert!(output.status.success(), "room.create {name:?}: {output:?}");
            let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(response["ok"], true, "room.create {name:?}: {output:?}");
        }))
        .map_err(panic_text)
    };

    // A failed sibling startup drops its partial fixture before this probe.
    let first_control = create_room(&first, "first-after-sibling-start");
    let second_control = second
        .as_ref()
        .ok()
        .map(|client| create_room(client, "second-independent"));
    let second_start = second.as_ref().map(|_| ()).map_err(Clone::clone);
    drop(second);
    let first_after_drop = create_room(&first, "first-after-sibling-drop");

    assert!(
        second_start.is_ok()
            && first_control.is_ok()
            && second_control.as_ref().is_some_and(Result::is_ok)
            && first_after_drop.is_ok()
            && first_base != second_base,
        "same-clock fixtures: first={first_base:?}, second={second_base:?}; \
         second startup={second_start:?}; first control={first_control:?}; \
         second control={second_control:?}; first after drop={first_after_drop:?}"
    );
}

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
