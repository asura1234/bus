use super::*;

#[test]
fn input_splits_text_keys_and_pastes() {
    let mut pending = b"ab\x1b[200~line\nmore\x1b[201~\x7f\r\x1b[13u\x1b[A\x03".to_vec();
    assert_eq!(
        drain_input(&mut pending),
        vec![
            Input::Text("abline\nmore".into()),
            Input::Backspace,
            Input::Enter,
            Input::Enter,
            Input::Interrupt,
        ]
    );
    assert!(pending.is_empty());
}

#[test]
fn unfinished_sequences_wait_for_more_bytes() {
    let mut pending = b"x\x1b[200~half".to_vec();
    assert_eq!(drain_input(&mut pending), vec![Input::Text("x".into())]);
    assert_eq!(pending, b"\x1b[200~half");
    let mut lone = b"\x1b".to_vec();
    assert!(drain_input(&mut lone).is_empty());
    let mut alt = b"\x1bb\x01z".to_vec();
    assert_eq!(drain_input(&mut alt), vec![Input::Text("z".into())]);
}

#[test]
fn work_prompts_and_provider_names() {
    assert_eq!(work_seconds("please fake:work 3 now"), Some(3));
    assert_eq!(work_seconds("fake:work 9999"), Some(600));
    assert_eq!(work_seconds("just hello"), None);
    assert!(PROVIDER_NAMES.contains(&"claude"));
    assert_eq!(
        settings_path(&["--settings".into(), "/x.json".into()]),
        Some("/x.json".into())
    );
    assert_eq!(settings_path(&[]), None);
    assert_eq!(pseudo_uuid().len(), 36);
}

#[test]
fn install_links_every_provider_name() {
    let dir = std::env::temp_dir().join(format!("bus-tui-fake-{}", std::process::id()));
    let binary = std::env::current_exe().unwrap();
    install(&dir, &binary).unwrap();
    for name in PROVIDER_NAMES {
        assert!(dir.join(exe_name(name)).exists(), "{name} missing");
    }
    // Installing again replaces the links.
    install(&dir, &binary).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}
