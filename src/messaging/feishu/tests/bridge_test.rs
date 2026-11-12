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
