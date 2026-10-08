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
    client.observe("# margin-first");
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
    client.observe("# margin-other");
    assert!(
        client.screen.margin().trim().is_empty(),
        "{:?}",
        client.screen.margin()
    );
}
