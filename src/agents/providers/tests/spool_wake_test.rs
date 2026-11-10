use super::ready_launch;

#[test]
fn callback_wake_hint_accepts_only_a_bounded_launch_key() {
    assert_eq!(
        ready_launch(br#"{"callback_wake":"launch-123_abc"}"#).as_deref(),
        Some("launch-123_abc")
    );
    for bytes in [
        br#"{"callback_wake":"../launch"}"#.as_slice(),
        br#"{"callback_wake":""}"#,
        br#"{"callback_wake":"launch","payload":"untrusted"}"#,
        br#"{"id":"control","method":"message.send"}"#,
        b"not json",
    ] {
        assert!(ready_launch(bytes).is_none());
    }
    assert!(ready_launch(
        &serde_json::to_vec(&serde_json::json!({"callback_wake":"a".repeat(129)})).unwrap()
    )
    .is_none());
}
