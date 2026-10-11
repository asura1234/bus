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
    let env = scratch_env(&paths, "tok", &inherited);
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
    assert_eq!(get(OWNER_ENV), Some(OsString::from("tok")));
    let without_path = scratch_env(&paths, "tok", &[]);
    assert!(without_path.iter().any(|(k, _)| k == "PATH"));
}

#[test]
fn session_directories_are_created_marked_and_removed() {
    let root = temp_root("dirs");
    let paths = SessionPaths::new(&root, "s1");
    assert!(!paths.is_driver_dir());
    assert!(paths.remove().is_ok());
    create_private_dir(&root).unwrap();
    let claim = Claim::new("s1");
    paths.claim(&claim).unwrap();
    assert!(paths.is_driver_dir());
    assert_eq!(paths.read_claim(), Some(claim.clone()));
    // A name is claimed once; the second claim fails instead of sharing it.
    assert!(paths.claim(&Claim::new("s1")).is_err());
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

/// True once `check` holds, within two seconds. Another test thread that
/// spawns a child while a lock file is open hands the child that open file
/// for a moment (fork to exec), and a flock belongs to the open file, so a
/// released lock can stay held for a few milliseconds in this process.
fn eventually(mut check: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !check() {
        if std::time::Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

#[test]
fn a_session_is_owned_by_its_start_then_by_its_host_lock() {
    let root = temp_root("owner");
    let paths = SessionPaths::new(&root, "s1");
    create_private_dir(&root).unwrap();
    // Claimed by a live `start` (this process): owned while it starts.
    let claim = Claim::new("s1");
    paths.claim(&claim).unwrap();
    assert!(paths.owned());
    assert_eq!(owned_sessions(&root), vec!["s1".to_owned()]);
    // A host with the wrong token touches nothing.
    assert!(paths.take_ownership("wrong").is_err());
    // The claimant is gone: owned only while the host holds its lock.
    let orphan = Claim {
        token: claim.token.clone(),
        claimant: u32::MAX / 2,
    };
    std::fs::write(
        paths.dir.join(MARKER),
        serde_json::to_string(&orphan).unwrap(),
    )
    .unwrap();
    assert!(eventually(|| !paths.owned()));
    let mut lock = None;
    assert!(eventually(|| {
        lock = paths.take_ownership(&claim.token).ok();
        lock.is_some()
    }));
    assert!(paths.owned());
    // A second host for the same session is refused.
    assert!(paths.take_ownership(&claim.token).is_err());
    drop(lock);
    assert!(eventually(|| !paths.owned()));
    assert!(eventually(|| owned_sessions(&root).is_empty()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_second_start_never_clears_a_same_named_session_whose_host_is_still_starting() {
    // A host writes session.json only after its Bus answers `bus state`;
    // until then its `start` holds the claim.
    let root = temp_root("starting");
    create_private_dir(&root).unwrap();
    let paths = SessionPaths::new(&root, "s1");
    paths.claim(&Claim::new("s1")).unwrap();
    let state = paths.data_dir().join("state.json");
    std::fs::write(&state, b"{}").unwrap();
    let second = super::super::prepare_target(&root, &paths, &[]);
    let kept = state.exists();
    let _ = std::fs::remove_dir_all(&root);
    assert!(second.is_err() && kept, "{second:?}");
}

#[test]
fn refuses_a_target_whose_herdr_server_still_answers() {
    // Bus always runs `--session bus`, so its herdr API socket lives under sessions/bus.
    let root = temp_root("herdr");
    let data = root.join("data");
    let api = data.join("herdr-config").join("sessions").join("bus");
    create_private_dir(&api).unwrap();
    let _listener = crate::platform::ipc::bind_local_listener(&api.join("herdr.sock")).unwrap();
    let result = ensure_not_live_bus(&data);
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        result.is_err(),
        "a live herdr API socket in the target was not detected"
    );
}

#[test]
fn teardown_signals_only_processes_carrying_the_session_token() {
    let root = temp_root("sweep");
    create_private_dir(&root).unwrap();
    // Both hold the directory as their cwd; only one was started by the session.
    let spawn = |token: Option<&str>| {
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("30").current_dir(&root);
        if let Some(token) = token {
            command.env(OWNER_ENV, token);
        }
        command.spawn().unwrap()
    };
    let mut owned = spawn(Some("tok-a"));
    let mut foreign = spawn(None);
    let mut other = spawn(Some("tok-b"));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while pids_holding(&root).len() < 3 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let found = pids_owned(&root, "tok-a");
    let readable =
        crate::platform::process_environment(owned.id()).is_some_and(|env| !env.is_empty());
    for child in [&mut owned, &mut foreign, &mut other] {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(!found.contains(&foreign.id()), "{found:?}");
    assert!(!found.contains(&other.id()), "{found:?}");
    // macOS hides a platform binary's environment (and sandboxes may deny
    // it); then nothing is signalled, which is the safe side.
    if readable {
        assert_eq!(found, vec![owned.id()]);
    }
}

#[test]
fn a_driver_root_must_be_isolated_before_start_touches_it() {
    // An inherited live Bus data dir used as the root: refused, untouched.
    let live = temp_root("rootlive");
    create_private_dir(&live).unwrap();
    std::fs::write(live.join("state.json"), b"{}").unwrap();
    let _listener = crate::platform::ipc::bind_local_listener(&live.join("control.sock")).unwrap();
    let before = std::fs::read_dir(&live).unwrap().count();
    let pointing = vec![("BUS_DATA_DIR".to_owned(), live.clone().into_os_string())];
    assert!(ensure_root_isolated(&live, &pointing, &[]).is_err());
    assert!(ensure_root_isolated(&live, &[], &[]).is_err());
    assert_eq!(std::fs::read_dir(&live).unwrap().count(), before);
    assert!(!live.join(ROOT_LOCK).exists());
    let _ = std::fs::remove_dir_all(&live);
    // A Bus home, or a root inside one, is refused.
    let home = PathBuf::from("/Users/me/.local/share/bus");
    assert!(ensure_root_isolated(&home.join("x"), &[], std::slice::from_ref(&home)).is_err());
    // A missing root, or one the driver made, is fine.
    let fresh = temp_root("rootfresh");
    assert!(ensure_root_isolated(&fresh, &[], &[]).is_ok());
    create_private_dir(&fresh).unwrap();
    drop(lock_root(&fresh).unwrap());
    assert!(ensure_root_isolated(&fresh, &[], &[]).is_ok());
    let _ = std::fs::remove_dir_all(&fresh);
}

/// A directory and a second spelling of it: through a symlink, and on macOS
/// also `/private/tmp` for `/tmp`.
fn aliased_dirs(tag: &str) -> (PathBuf, Vec<(PathBuf, PathBuf)>) {
    let base = temp_root(tag);
    let real = base.join("real");
    create_private_dir(&real).unwrap();
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut pairs = vec![(real.clone(), link)];
    let canonical = real.canonicalize().unwrap();
    if canonical != real {
        pairs.push((canonical, real));
    }
    (base, pairs)
}

#[test]
fn resolving_follows_aliases_without_creating_anything() {
    let (base, pairs) = aliased_dirs("resolve");
    for (real, alias) in &pairs {
        let canonical = real.canonicalize().unwrap();
        assert_eq!(resolve_existing(alias).unwrap(), canonical);
        let missing = alias.join("not").join("yet");
        assert_eq!(
            resolve_existing(&missing).unwrap(),
            canonical.join("not").join("yet")
        );
        assert!(!real.join("not").exists());
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn root_guards_see_through_path_aliases() {
    let (base, pairs) = aliased_dirs("alias");
    let _listener =
        crate::platform::ipc::bind_local_listener(&pairs[0].0.join("control.sock")).unwrap();
    for (real, alias) in &pairs {
        // A root inside the inherited data dir, spelled differently.
        let root = alias.join("driver");
        let locator = vec![("BUS_DATA_DIR".to_owned(), real.clone().into_os_string())];
        assert!(
            ensure_env_does_not_point_at(&root, &locator).is_err(),
            "{} vs {}",
            root.display(),
            real.display()
        );
        assert!(ensure_outside_bus_homes(&root, std::slice::from_ref(real)).is_err());
        // Even with no locator or home, a live Bus above the root refuses it.
        let refusal = ensure_root_isolated(&root, &[], &[]).unwrap_err();
        assert!(refusal.contains("live Bus"), "{refusal}");
        assert!(!root.exists());
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_path_that_cannot_be_resolved_is_an_error_not_its_spelling() {
    let base = temp_root("unresolved");
    create_private_dir(&base).unwrap();
    // `m` is missing, so `m/..` cannot be resolved without guessing.
    let dotted = base.join("m").join("..").join("live");
    assert!(resolve_existing(&dotted).is_err());
    assert!(!base.join("m").exists());
    let locator = vec![("BUS_DATA_DIR".to_owned(), base.clone().into_os_string())];
    assert!(ensure_env_does_not_point_at(&dotted, &locator).is_err());
    assert!(ensure_outside_bus_homes(&dotted, &[]).is_ok());
    assert!(ensure_outside_bus_homes(&dotted, std::slice::from_ref(&base)).is_err());
    assert!(ensure_root_isolated(&dotted, &[], &[]).is_err());
    assert!(resolve_existing(Path::new("relative")).is_err());
    let _ = std::fs::remove_dir_all(&base);
}
