use super::*;

#[test]
fn dashboard_service_reuses_one_verified_endpoint_and_recovers_a_stopped_owner() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-service");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("state.json"), b"untouched state").unwrap();
    let started = start(&root);
    assert!(started.is_ok(), "service should start");
    let Started::Owned(first) = started.unwrap() else {
        panic!("first service owns listener")
    };
    let listener = first.listener.try_clone().unwrap();
    let token = first.endpoint.token.clone();
    let url = first.endpoint.url.clone();
    let health_root = root.clone();
    let health = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        super::super::serve_connection(&mut stream, &health_root, &authority, &token).unwrap();
    });
    let capture = crate::utils::logging::test_capture::Capture::default();
    let mut outcome = None;
    capture.run(|| outcome = Some(start(&root)));
    let outcome = outcome.unwrap();
    assert!(outcome.is_ok(), "{}", capture.text());
    let Started::Existing(existing) = outcome.unwrap() else {
        panic!("second caller reuses service")
    };
    assert_eq!(existing.url, url);
    health.join().unwrap();
    drop(first);
    let Started::Owned(replacement) = start(&root).unwrap() else {
        panic!("stopped owner releases lock")
    };
    assert_ne!(replacement.endpoint.token, existing.token);
    assert_eq!(
        std::fs::read(root.join("state.json")).unwrap(),
        b"untouched state"
    );
    drop(replacement);
    std::fs::remove_dir_all(root).unwrap();
}
