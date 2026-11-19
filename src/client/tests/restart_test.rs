use super::*;

#[test]
fn restart_admission_stays_closed_after_cli_exit_until_handoff_ends() {
    let root = crate::utils::test_temp::unique_temp_path("restart-key-admission");
    let lock = restart_record::restart_lock_path(&root);
    let pending = AtomicBool::new(false);
    let cli = PendingRestart::try_begin(&pending, &root).expect("first key starts the CLI");
    assert!(PendingRestart::try_begin(&pending, &root).is_none());

    let helper = crate::platform::process_instance(std::process::id()).unwrap();
    restart_record::write_json(
        &lock,
        &LivePid {
            pid: helper.pid,
            birth: helper.birth,
        },
    )
    .unwrap();
    drop(cli);
    let during_handoff = PendingRestart::try_begin(&pending, &root).is_some();
    std::fs::remove_file(&lock).unwrap();
    let after_handoff = PendingRestart::try_begin(&pending, &root).is_some();

    restart_record::write_json(
        &lock,
        &LivePid {
            pid: helper.pid,
            birth: helper.birth.wrapping_add(1),
        },
    )
    .unwrap();
    let after_owner_loss = PendingRestart::try_begin(&pending, &root).is_some();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(!during_handoff, "second key would start another build");
    assert!(after_handoff, "new key can start after handoff ends");
    assert!(after_owner_loss, "stale lock must not disable the key");
}
