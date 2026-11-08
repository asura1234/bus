#[test]
fn record_serving_build_ties_the_stamp_to_this_process() {
    let root = crate::utils::test_temp::unique_temp_path("serving-build");
    std::fs::create_dir_all(&root).unwrap();
    let sha = "cd".repeat(20);
    super::record_serving_build(Some(&root), &sha);
    let stamp = crate::utils::restart_record::read_build_stamp(
        &crate::utils::restart_record::build_path(&root),
    )
    .unwrap();
    let live = crate::platform::process_instance(std::process::id()).unwrap();
    assert_eq!(stamp.sha, sha);
    assert_eq!(stamp.pid, Some(live.pid));
    assert_eq!(stamp.birth, Some(live.birth));
    assert!(stamp.binary.is_some());
    std::fs::remove_dir_all(&root).unwrap();
}
