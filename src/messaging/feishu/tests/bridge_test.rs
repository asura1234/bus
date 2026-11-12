use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn feishu_bridge_send_uses_literal_v1_argv_and_requires_both_external_ids() {
    let root = crate::utils::test_temp::unique_temp_path("feishu-bridge-contract");
    std::fs::create_dir_all(&root).unwrap();
    let script = root.join("feishu-bridge");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"${0%/*}/args\"\ncat \"${0%/*}/response\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client::default();
    let text = "@literal - text $HOME `command` 界";
    for (response, accepted) in [
        (r#"{"message_id":"om_out","chat_id":"oc_dm"}"#, true),
        (r#"{"message_id":"om_out"}"#, false),
        (r#"{"chat_id":"oc_dm"}"#, false),
        (r#"{"message_id":"","chat_id":"oc_dm"}"#, false),
        (r#"{"message_id":"out","chat_id":"oc_dm"}"#, false),
        (r#"{"message_id":"om_out","chat_id":"oc_"}"#, false),
        (r#"{"message_id":"om_out","chat_id":"oc_ dm"}"#, false),
    ] {
        std::fs::write(root.join("response"), response).unwrap();
        let result = client.send(
            "ou_developer",
            script.to_str(),
            text,
            "bus-stable-request",
            &Cancellation::default(),
        );
        assert_eq!(result.is_ok(), accepted, "{response}: {result:?}");
        if accepted {
            let sent = result.unwrap();
            assert_eq!(sent.message_id, "om_out");
            assert_eq!(sent.chat_id, "oc_dm");
        }
    }
    let args = std::fs::read_to_string(root.join("args")).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "send",
            "--open-id",
            "ou_developer",
            "--text",
            text,
            "--idempotency-key",
            "bus-stable-request"
        ]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn feishu_bridge_discovery_requires_an_executable_plugin_and_pins_its_absolute_path() {
    let root = crate::utils::test_temp::unique_temp_path("feishu-bridge-discovery");
    std::fs::create_dir_all(&root).unwrap();
    let client = Client::on_path(&root, std::time::Duration::from_secs(10));
    assert_eq!(client.program(None), None);
    let legacy = root.join("lark-cli");
    std::fs::write(&legacy, "#!/bin/sh\nexit 77\n").unwrap();
    std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        client.program(None),
        None,
        "the removed transport cannot enable Feishu"
    );
    let bridge = root.join("feishu-bridge");
    std::fs::write(&bridge, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&bridge, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        client.program(None),
        None,
        "a non-executable file is not an installed plugin"
    );
    std::fs::set_permissions(&bridge, std::fs::Permissions::from_mode(0o700)).unwrap();
    let expected = std::fs::canonicalize(&bridge).unwrap();
    assert_eq!(client.program(None), Some(expected.clone()));
    assert_eq!(Client::default().program(bridge.to_str()), Some(expected));
    assert_eq!(
        client.program(Some("/missing/feishu-bridge")),
        None,
        "an explicit missing path must not fall back to PATH"
    );
    assert_eq!(
        client.program(Some("feishu-bridge")),
        None,
        "configured paths must be absolute"
    );
    assert_eq!(
        Client::default().program(None),
        None,
        "unit tests cannot discover the real installed plugin"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn feishu_bridge_invalid_recipient_or_key_never_launches_the_plugin() {
    let root = crate::utils::test_temp::unique_temp_path("feishu-bridge-invalid-input");
    std::fs::create_dir_all(&root).unwrap();
    let script = root.join("feishu-bridge");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf called > \"${0%/*}/called\"\nexit 7\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let client = Client::default();
    let long_key = "x".repeat(51);
    for (recipient, key) in [
        ("oc_group", "valid"),
        ("ou_", "valid"),
        ("ou_developer", ""),
        ("ou_developer", long_key.as_str()),
        ("ou_developer", "界"),
    ] {
        assert!(client
            .send(
                recipient,
                script.to_str(),
                "literal",
                key,
                &Cancellation::default()
            )
            .is_err());
    }
    assert!(!root.join("called").exists());
    std::fs::remove_dir_all(root).unwrap();
}
