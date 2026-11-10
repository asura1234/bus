use super::*;

fn temp_dir(label: &str) -> PathBuf {
    let dir = crate::utils::test_temp::unique_temp_path(&format!("bus-fs-{label}"));
    create_private_directory(&dir).unwrap();
    dir
}

#[test]
fn failed_write_removes_partial_temp_and_preserves_destination() {
    let dir = temp_dir("failed-write");
    let path = dir.join("state.json");
    atomic_write(&path, b"original").unwrap();
    let error = atomic_write_with(&path, |file| {
        file.write_all(b"partial")?;
        Err(io::Error::other("injected write failure"))
    })
    .unwrap_err();
    assert_eq!(error.source.to_string(), "injected write failure");
    assert!(!error.path.exists());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_replace_removes_temp_and_preserves_destination() {
    let dir = temp_dir("failed-replace");
    let path = dir.join("directory");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep"), b"original").unwrap();
    let error = atomic_write_detailed(&path, b"replacement").unwrap_err();
    assert_eq!(error.path, path);
    assert_eq!(std::fs::read(path.join("keep")).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_writes_use_distinct_temps_and_leave_complete_bytes() {
    let dir = temp_dir("concurrent");
    let path = dir.join("state.json");
    std::thread::scope(|scope| {
        for index in 0..8_u8 {
            let path = &path;
            scope.spawn(move || atomic_write(path, &[index; 1024]).unwrap());
        }
    });
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 1024);
    assert!(bytes.iter().all(|byte| *byte == bytes[0]));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exclusive_file_lock_is_released_on_drop() {
    let dir = temp_dir("lock");
    let path = dir.join("lock");
    create_private_state_file(&path).unwrap();
    let lease = lock_file(&path).unwrap();
    assert_eq!(
        lock_file(&path).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(lease);
    assert!(lock_file(&path).is_ok());
    assert!(lock_file(&dir.join("missing")).is_err());
    assert!(!dir.join("missing").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exclusive_file_lock_releases_while_a_duplicate_descriptor_stays_open() {
    let dir = temp_dir("lock-inherited-descriptor");
    let path = dir.join("lock");
    create_private_state_file(&path).unwrap();
    let lease = lock_file(&path).unwrap();
    // Fork duplicates this open file description until the child execs.
    let inherited = lease.try_clone().unwrap();
    assert_eq!(
        lock_file(&path).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(lease);
    let next = lock_file(&path).expect("the completed critical section must release its lock");
    drop(inherited);
    assert_eq!(
        lock_file(&path).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(next);
    assert!(lock_file(&path).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn private_files_and_directories_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("permissions");
    let path = dir.join("state.json");
    atomic_write(&path, b"first").unwrap();
    atomic_write(&path, b"second").unwrap();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn launchers_and_executables_are_revealed_not_opened() {
    let dir = temp_dir("launches");
    let document = dir.join("notes.md");
    std::fs::write(&document, "x").unwrap();
    let bundle = dir.join("Tool.app");
    std::fs::create_dir_all(&bundle).unwrap();
    let folder = dir.join("docs");
    std::fs::create_dir_all(&folder).unwrap();
    assert!(!launches_code(&document), "a document opens");
    assert!(!launches_code(&folder), "a folder opens");
    assert!(launches_code(&bundle), "an app bundle runs");
    for name in [
        "setup.EXE",
        "run.command",
        "x.ps1",
        "a.desktop",
        "b.AppImage",
        "c.lnk",
    ] {
        std::fs::write(dir.join(name), "x").unwrap();
        assert!(launches_code(&dir.join(name)), "{name}");
    }
    assert!(launches_code(&dir.join("gone.md")), "unresolvable: reveal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("build");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(launches_code(&script), "an executable file runs");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!launches_code(&folder), "a searchable folder still opens");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn script_host_and_shortcut_files_are_revealed_not_opened() {
    // Each of these runs code, or follows a link to code, when its default
    // app opens it: shortcuts, Terminal settings, Finder and Internet
    // location files, and on Windows script hosts, mshta and Control Panel.
    let dir = temp_dir("script-hosts");
    let mut names = vec![
        "a.url",
        "a.terminal",
        "a.fileloc",
        "a.inetloc",
        "a.webloc",
        "a.workflow",
    ];
    if cfg!(windows) {
        names.extend([
            "a.vbs", "a.vbe", "a.js", "a.jse", "a.wsf", "a.wsh", "a.hta", "a.cpl", "a.pif",
        ]);
    }
    for name in names {
        let path = dir.join(name);
        std::fs::write(&path, "x").unwrap();
        assert!(launches_code(&path), "{name}");
    }
    // Elsewhere a script source is a document an editor opens.
    #[cfg(unix)]
    {
        let source = dir.join("app.js");
        std::fs::write(&source, "x").unwrap();
        assert!(!launches_code(&source), "app.js opens");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn links_are_judged_by_what_they_point_at() {
    use std::os::unix::fs::symlink;
    let dir = temp_dir("app-symlink");
    let bundle = dir.join("Tool.app");
    std::fs::create_dir(&bundle).unwrap();
    let shortcut = dir.join("Tool");
    symlink(&bundle, &shortcut).unwrap();
    let document = dir.join("notes.md");
    std::fs::write(&document, "x").unwrap();
    let alias = dir.join("Notes");
    symlink(&document, &alias).unwrap();
    assert!(
        launches_code(&shortcut),
        "an app through a symlink is revealed"
    );
    assert!(!launches_code(&alias), "a document through a symlink opens");
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn bus_cli_alias_refresh_from_its_own_path_keeps_the_executable_reachable() {
    let root = crate::utils::test_temp::unique_temp_path("bus-alias-self-refresh");
    let real = root.join("real/bus");
    let alias = root.join("cli-bin/bus");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
    std::fs::write(&real, b"image").unwrap();
    crate::platform::fs::refresh_executable_alias(&alias, &real).unwrap();
    // A CLI/server invoked through the alias can report this path as its image.
    let _ = crate::platform::fs::refresh_executable_alias(&alias, &alias);
    assert!(
        std::fs::canonicalize(&alias).is_ok(),
        "alias refresh must not publish a self-loop"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn running_executable_keeps_a_missing_image_path_nonfatal() {
    let root = temp_dir("missing-running-image");
    let image = root.join("bus (deleted)");
    assert!(!image.exists());
    assert_eq!(
        resolve_running_executable(&image).expect("a replaced image must not block pane launch"),
        image
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bus_cli_alias_refuses_images_inside_its_directory() {
    let root = temp_dir("alias-containment");
    let directory = root.join("cli-bin");
    create_private_directory(&directory).unwrap();
    let inside = directory.join("another-image");
    std::fs::write(&inside, b"image").unwrap();
    let alias = directory.join(format!("bus{}", std::env::consts::EXE_SUFFIX));
    assert!(refresh_executable_alias(&alias, &inside).is_err());
    assert!(!alias.exists());
    assert_eq!(std::fs::read(&inside).unwrap(), b"image");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bus_cli_directory_is_absolute_before_panes_change_working_directory() {
    let directory = bus_cli_directory(Path::new("relative-session")).unwrap();
    assert!(directory.is_absolute());
    assert!(directory.starts_with(
        std::env::current_dir()
            .unwrap()
            .join("relative-session/cli-bin")
    ));
}

#[cfg(windows)]
#[test]
fn native_bus_alias_forwards_without_replacing_its_running_image() {
    const CHILD: &str = "TEST_CLI_ALIAS_CHILD_ROOT";
    if let Some(root) = std::env::var_os(CHILD) {
        if let Some(status) = forward_cli_alias().unwrap() {
            assert!(status.success());
            return;
        }
        let root = PathBuf::from(root);
        let origin = running_executable().unwrap();
        assert_eq!(
            origin,
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        let next = bus_cli_directory(&root).unwrap();
        private_dir(&next).unwrap();
        refresh_executable_alias(&next.join("bus.exe"), &origin).unwrap();
        std::fs::write(
            root.join("child-origin"),
            origin
                .as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(next, root.join("cli-bin"));
        return;
    }
    use std::os::windows::ffi::OsStrExt;
    let root = temp_dir("native-cli-origin");
    let directory = bus_cli_directory(&root).unwrap();
    private_dir(&directory).unwrap();
    let alias = directory.join("bus.exe");
    let origin = running_executable().unwrap();
    refresh_executable_alias(&alias, &origin).unwrap();
    assert_eq!(resolve_running_executable(&alias).unwrap(), origin);
    let status = std::process::Command::new(&alias)
        .args([
            "--exact",
            "platform::fs::tests::native_bus_alias_forwards_without_replacing_its_running_image",
            "--nocapture",
        ])
        .env(CHILD, &root)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "alias-started server must not replace its running image"
    );
    assert!(root.join("child-origin").exists());
    std::fs::write(directory.join(".bus-origin"), b"odd").unwrap();
    assert!(resolve_running_executable(&alias).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
