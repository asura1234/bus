use super::*;

fn temp_root(tag: &str) -> PathBuf {
    let root = PathBuf::from("/tmp").join(format!("bus-tui-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

#[test]
fn names_are_short_lowercase_identifiers() {
    assert!(validate_name("s1").is_ok());
    assert!(validate_name("self-check-2").is_ok());
    for bad in ["", "-x", "Upper", "a/b", "seventeen-chars-x"] {
        assert!(validate_name(bad).is_err(), "{bad:?} accepted");
    }
}

#[test]
fn refuses_a_target_a_live_bus_already_serves() {
    // A listening control socket is what a running Bus leaves in its data dir.
    let root = temp_root("live");
    let data = root.join("data");
    create_private_dir(&data).unwrap();
    assert!(ensure_not_live_bus(&data).is_ok());
    let _listener = crate::platform::ipc::bind_local_listener(&data.join("control.sock")).unwrap();
    let refusal = ensure_not_live_bus(&data).unwrap_err();
    assert!(refusal.contains("live Bus session"), "{refusal}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn refuses_inherited_variables_that_point_at_the_target() {
    let target = Path::new("/tmp/bus-tui/s1");
    let pointing = vec![(
        "BUS_DATA_DIR".to_owned(),
        OsString::from("/tmp/bus-tui/s1/data"),
    )];
    assert!(ensure_env_does_not_point_at(target, &pointing).is_err());
    let socket = vec![(
        "HERDR_SOCKET_PATH".to_owned(),
        OsString::from("/tmp/bus-tui/s1/data/herdr-config/herdr.sock"),
    )];
    assert!(ensure_env_does_not_point_at(target, &socket).is_err());
    // Variables naming another session are fine: they are scrubbed, not followed.
    let elsewhere = vec![
        (
            "BUS_DATA_DIR".to_owned(),
            OsString::from("/Users/me/.local/share/bus/abc"),
        ),
        ("PATH".to_owned(), OsString::from("/tmp/bus-tui/s1")),
        ("HERDR_CONFIG_PATH".to_owned(), OsString::new()),
    ];
    assert!(ensure_env_does_not_point_at(target, &elsewhere).is_ok());
}

#[test]
fn refuses_targets_inside_a_real_bus_home() {
    let home = PathBuf::from("/Users/me/.local/share/bus");
    assert!(ensure_outside_bus_homes(&home.join("x"), std::slice::from_ref(&home)).is_err());
    assert!(ensure_outside_bus_homes(Path::new("/tmp/bus-tui/s1"), &[home]).is_ok());
    assert!(!bus_homes().is_empty());
}

#[test]
fn refuses_the_developers_target_debug_binary() {
    assert!(ensure_binary_allowed(Path::new("/r/target/debug/bus"), false).is_err());
    assert!(ensure_binary_allowed(Path::new("/r/target/debug/bus"), true).is_ok());
    assert!(ensure_binary_allowed(Path::new("/r/target/tui-driver/debug/bus"), false).is_ok());
    assert!(ensure_binary_allowed(Path::new("/usr/local/bin/bus"), false).is_ok());
}

#[test]
fn scratch_env_keeps_basics_and_drops_every_inherited_bus_variable() {
    let paths = SessionPaths::new(Path::new("/tmp/bus-tui"), "s1");
    let inherited: Vec<(String, OsString)> = [
        ("PATH", "/usr/bin"),
        ("HOME", "/Users/me"),
        ("LC_ALL", "C"),
        ("BUS_DATA_DIR", "/Users/me/.local/share/bus/live"),
        ("BUS_SESSION_ID", "live"),
        ("HERDR_SOCKET_PATH", "/live/herdr.sock"),
        ("CLAUDECODE", "1"),
        ("TERM_PROGRAM", "ghostty"),
        ("TMUX", "/tmp/tmux"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), OsString::from(v)))
    .collect();
    let env = scratch_env(&paths, &inherited);
    let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    assert_eq!(
        get("BUS_DATA_DIR"),
        Some(OsString::from("/tmp/bus-tui/s1/data"))
    );
    assert!(get("BUS_SESSION_ID").is_none());
    assert!(get("HERDR_SOCKET_PATH").is_none());
    assert!(get("CLAUDECODE").is_none());
    assert!(get("TERM_PROGRAM").is_none());
    assert!(get("TMUX").is_none());
    assert_eq!(get("HOME"), Some(OsString::from("/Users/me")));
    assert_eq!(get("LC_ALL"), Some(OsString::from("C")));
    assert!(get("PATH")
        .unwrap()
        .to_string_lossy()
        .starts_with("/tmp/bus-tui/s1/bin"));
    assert_eq!(get("TERM"), Some(OsString::from("xterm-256color")));
    assert!(env.iter().all(|(k, _)| !k.starts_with("HERDR_")));
    let without_path = scratch_env(&paths, &[]);
    assert!(without_path.iter().any(|(k, _)| k == "PATH"));
}

#[test]
fn session_directories_are_created_marked_and_removed() {
    let root = temp_root("dirs");
    let paths = SessionPaths::new(&root, "s1");
    assert!(!paths.is_driver_dir());
    assert!(paths.remove().is_ok());
    paths.create().unwrap();
    assert!(paths.is_driver_dir());
    assert!(paths.read_info().is_none());
    let info = SessionInfo {
        name: "s1".into(),
        host_pid: std::process::id(),
        bus_pid: None,
        data_dir: paths.data_dir(),
        run_dir: root.join("run"),
        binary: "/bin/bus".into(),
        size: (80, 24),
    };
    paths.write_info(&info).unwrap();
    assert_eq!(paths.read_info(), Some(info.clone()));
    assert!(session_alive(&paths));
    assert_eq!(live_sessions(&root), vec![info]);
    paths.remove().unwrap();
    assert!(!paths.dir.exists());
    assert!(live_sessions(&root).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn processes_and_run_roots() {
    assert!(process_alive(std::process::id()));
    assert!(!process_alive(u32::MAX / 2));
    let checkout = find_bus_checkout(Path::new(env!("CARGO_MANIFEST_DIR")));
    assert!(checkout.is_some());
    assert!(
        default_runs_root(Path::new(env!("CARGO_MANIFEST_DIR"))).ends_with("temp/tui-driver/runs")
    );
    assert!(default_runs_root(Path::new("/")).ends_with("bus/tui-runs"));
    assert!(pids_holding(Path::new("/nonexistent-bus-tui-dir")).is_empty());
    assert!(driver_root().ends_with("bus-tui"));
}
