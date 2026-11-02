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

#[test]
fn dashboard_endpoint_discovery_retains_the_record_path_and_parse_cause() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-discovery");
    std::fs::create_dir_all(&root).unwrap();
    let Started::Owned(owner) = start(&root).unwrap() else {
        panic!("owns service")
    };
    std::fs::write(
        root.join("dashboard-runtime")
            .join(env!("BUS_BUILD_SHA"))
            .join("endpoint.json"),
        "not json",
    )
    .unwrap();
    let error = match start(&root) {
        Err(error) => error,
        Ok(_) => panic!("corrupt discovery must not be reused"),
    };
    assert!(
        error.to_string().contains("endpoint.json") && error.to_string().contains("JSON"),
        "{error}"
    );
    drop(owner);
    std::fs::remove_dir_all(root).unwrap();
}

// A matching discovery nonce alone does not prove the process embeds this page.
#[test]
fn dashboard_health_rejects_other_or_unidentified_builds() {
    for build in [None, Some("a-different-build"), Some(env!("BUS_BUILD_SHA"))] {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = Endpoint {
            url: format!("http://{}/", listener.local_addr().unwrap()),
            token: "verified-nonce".into(),
            build: env!("BUS_BUILD_SHA").into(),
        };
        let body = serde_json::json!({"token":endpoint.token,"build":build}).to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let healthy = endpoint.healthy(env!("BUS_BUILD_SHA")).unwrap();
        server.join().unwrap();
        assert_eq!(
            healthy,
            build == Some(env!("BUS_BUILD_SHA")),
            "build={build:?}"
        );
    }
}

#[test]
fn dashboard_different_builds_get_their_own_embedded_page_service() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-generations");
    std::fs::create_dir_all(&root).unwrap();
    let Started::Owned(old) = start_for_build(&root, "another-build").unwrap() else {
        panic!("first build owns its listener")
    };
    let new = start(&root);
    let owns_own_page = matches!(&new, Ok(Started::Owned(_)));
    let detail = new.as_ref().err().map(ToString::to_string);
    drop(new);
    drop(old);
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        owns_own_page,
        "the current build reused or waited on another build: {detail:?}"
    );
}

#[test]
fn dashboard_old_build_retires_cooperatively_after_publication() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-retirement");
    std::fs::create_dir_all(&root).unwrap();
    let Started::Owned(mut old) = start_for_build(&root, "previous-build").unwrap() else {
        panic!("owns old build")
    };
    assert!(!old.retire_if_replaced(&root).unwrap());
    let Started::Owned(mut new) = start(&root).unwrap() else {
        panic!("owns current build")
    };
    // A launcher publishing or reopening holds this lock; retirement must defer.
    let publication = bus_io::lock(&root.join("dashboard-runtime/publication.lock")).unwrap();
    assert!(!old.retire_if_replaced(&root).unwrap());
    drop(publication);
    assert!(old.retire_if_replaced(&root).unwrap());
    assert!(!new.retire_if_replaced(&root).unwrap());
    assert!(old.lease.is_none());
    assert!(new.lease.is_some());
    drop(new);
    drop(old);
    std::fs::remove_dir_all(root).unwrap();
}
