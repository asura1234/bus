use super::*;

fn temp_dir(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bus-fs-{label}-{}-{unique}", std::process::id()));
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
