#[test]
fn wsl_marker_detection_matches_kernel_release_text() {
    assert!(text_indicates_wsl("5.15.167.4-microsoft-standard-WSL2"));
    assert!(text_indicates_wsl("4.4.0-19041-Microsoft"));
    assert!(!text_indicates_wsl("6.8.0-64-generic"));
    assert!(!text_indicates_wsl(""));
}

#[test]
fn clipboard_commands_prefer_wayland_when_available() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
        std::env::remove_var("DISPLAY");
    }
    let commands = clipboard_commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].program, "wl-copy");
}

#[test]
fn wl_copy_owner_does_not_block_clipboard_write() {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant, SystemTime};

    struct Cleanup {
        old_path: Option<OsString>,
        temp_dir: PathBuf,
        owner_pid: Option<i32>,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(pid) = self.owner_pid {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
            unsafe {
                match self.old_path.take() {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
                std::env::remove_var("HERDR_TEST_WL_COPY_MARKER");
                std::env::remove_var("HERDR_TEST_WL_COPY_PAYLOAD");
                std::env::remove_var("HERDR_TEST_WL_COPY_ARGS");
            }
            let _ = std::fs::remove_dir_all(&self.temp_dir);
        }
    }

    let _guard = env_lock().lock().unwrap();
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time should follow unix epoch")
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!(
        "herdr-fake-wl-copy-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let mut cleanup = Cleanup {
        old_path: std::env::var_os("PATH"),
        temp_dir: temp_dir.clone(),
        owner_pid: None,
    };
    let fake_wl_copy = temp_dir.join("wl-copy");
    let marker = temp_dir.join("owner-pid");
    let payload = temp_dir.join("payload");
    let args = temp_dir.join("args");
    std::fs::write(
        &fake_wl_copy,
        "#!/bin/sh\ncat > \"$HERDR_TEST_WL_COPY_PAYLOAD\"\nprintf '%s\\n' \"$@\" > \"$HERDR_TEST_WL_COPY_ARGS\"\nprintf '%s' \"$$\" > \"$HERDR_TEST_WL_COPY_MARKER\"\nexec sleep 30\n",
    )
    .expect("fake wl-copy should be written");
    let mut permissions = std::fs::metadata(&fake_wl_copy)
        .expect("fake wl-copy metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake_wl_copy, permissions)
        .expect("fake wl-copy should be executable");

    let test_path = match cleanup.old_path.as_ref() {
        Some(path) => {
            let mut paths = vec![temp_dir.clone()];
            paths.extend(std::env::split_paths(path));
            std::env::join_paths(paths).expect("test path should be valid")
        }
        None => temp_dir.clone().into_os_string(),
    };
    unsafe {
        std::env::set_var("PATH", test_path);
        std::env::set_var("HERDR_TEST_WL_COPY_MARKER", &marker);
        std::env::set_var("HERDR_TEST_WL_COPY_PAYLOAD", &payload);
        std::env::set_var("HERDR_TEST_WL_COPY_ARGS", &args);
    }

    let (result_tx, result_rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let command = ClipboardCommand {
            program: "wl-copy",
            args: &["--type", "text/plain;charset=utf-8"],
        };
        let _ = result_tx.send(run_clipboard_command(&command, b"clipboard text"));
    });

    let marker_deadline = Instant::now() + Duration::from_secs(2);
    while !marker.exists() && Instant::now() < marker_deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let owner_pid: i32 = std::fs::read_to_string(&marker)
        .expect("fake wl-copy should enter its clipboard-owner phase")
        .parse()
        .expect("owner pid should be numeric");
    cleanup.owner_pid = Some(owner_pid);
    let returned_while_owner_running = result_rx
        .recv_timeout(Duration::from_secs(2))
        .is_ok_and(|result| result);
    let actual_payload = std::fs::read(&payload).expect("fake wl-copy should record stdin");
    let actual_args = std::fs::read_to_string(&args).expect("fake wl-copy should record args");

    unsafe {
        libc::kill(owner_pid, libc::SIGTERM);
    }
    let reap_deadline = Instant::now() + Duration::from_secs(2);
    while process_exists(owner_pid as u32) && Instant::now() < reap_deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let owner_was_reaped = !process_exists(owner_pid as u32);
    cleanup.owner_pid = None;
    writer.join().expect("clipboard writer thread should join");
    drop(cleanup);

    assert!(
        returned_while_owner_running,
        "clipboard writes must return while wl-copy remains alive to own the selection"
    );
    assert_eq!(actual_payload, b"clipboard text");
    assert_eq!(actual_args, "--type\ntext/plain;charset=utf-8\n");
    assert!(
        owner_was_reaped,
        "wl-copy owner should be reaped after exit"
    );
}

#[test]
fn failed_wl_copy_uses_x11_fallback() {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Cleanup {
        old_path: Option<OsString>,
        old_wayland_display: Option<OsString>,
        old_display: Option<OsString>,
        temp_dir: PathBuf,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            unsafe {
                match self.old_path.take() {
                    Some(value) => std::env::set_var("PATH", value),
                    None => std::env::remove_var("PATH"),
                }
                match self.old_wayland_display.take() {
                    Some(value) => std::env::set_var("WAYLAND_DISPLAY", value),
                    None => std::env::remove_var("WAYLAND_DISPLAY"),
                }
                match self.old_display.take() {
                    Some(value) => std::env::set_var("DISPLAY", value),
                    None => std::env::remove_var("DISPLAY"),
                }
                std::env::remove_var("HERDR_TEST_XCLIP_PAYLOAD");
            }
            let _ = std::fs::remove_dir_all(&self.temp_dir);
        }
    }

    let _guard = env_lock().lock().unwrap();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should follow unix epoch")
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!(
        "herdr-failed-wl-copy-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cleanup = Cleanup {
        old_path: std::env::var_os("PATH"),
        old_wayland_display: std::env::var_os("WAYLAND_DISPLAY"),
        old_display: std::env::var_os("DISPLAY"),
        temp_dir: temp_dir.clone(),
    };
    let payload = temp_dir.join("xclip-payload");
    let fake_wl_copy = temp_dir.join("wl-copy");
    let fake_xclip = temp_dir.join("xclip");
    std::fs::write(&fake_wl_copy, "#!/bin/sh\n/bin/cat >/dev/null\nexit 7\n")
        .expect("fake wl-copy should be written");
    std::fs::write(
        &fake_xclip,
        "#!/bin/sh\n/bin/cat > \"$HERDR_TEST_XCLIP_PAYLOAD\"\n",
    )
    .expect("fake xclip should be written");
    for command in [&fake_wl_copy, &fake_xclip] {
        let mut permissions = std::fs::metadata(command)
            .expect("fake clipboard command metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(command, permissions)
            .expect("fake clipboard command should be executable");
    }

    unsafe {
        std::env::set_var("PATH", &temp_dir);
        std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
        std::env::set_var("DISPLAY", ":0");
        std::env::set_var("HERDR_TEST_XCLIP_PAYLOAD", &payload);
    }

    assert!(write_clipboard(b"clipboard fallback"));
    assert_eq!(
        std::fs::read(&payload).expect("xclip should record stdin"),
        b"clipboard fallback"
    );
    drop(cleanup);
}

#[test]
fn finite_clipboard_commands_report_exit_status() {
    let success = ClipboardCommand {
        program: "sh",
        args: &["-c", "cat >/dev/null"],
    };
    let failure = ClipboardCommand {
        program: "sh",
        args: &["-c", "cat >/dev/null; exit 7"],
    };

    assert!(run_clipboard_command(&success, b"clipboard text"));
    assert!(!run_clipboard_command(&failure, b"clipboard text"));
}

#[test]
fn clipboard_commands_include_x11_fallbacks() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("DISPLAY", ":0");
    }
    let commands = clipboard_commands();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].program, "xclip");
    assert_eq!(commands[1].program, "xsel");
}

#[test]
fn read_clipboard_image_with_spawned_command_reads_under_limit() {
    let mut command = Command::new("sh");
    command.arg("-c").arg("printf image");

    assert_eq!(
        read_clipboard_image_with_spawned_command_max(command, 16),
        Some(b"image".to_vec())
    );
}

#[test]
fn read_clipboard_image_with_spawned_command_rejects_over_limit() {
    let mut command = Command::new("sh");
    command.arg("-c").arg("printf oversized");

    assert_eq!(
        read_clipboard_image_with_spawned_command_max(command, 4),
        None
    );
}

#[test]
fn read_clipboard_image_rejects_xclip_text_served_for_image_target() {
    let _guard = env_lock().lock().unwrap();
    let temp_dir = std::env::temp_dir().join(format!("herdr-fake-xclip-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let fake_xclip = temp_dir.join("xclip");
    std::fs::write(&fake_xclip, "#!/bin/sh\nprintf '# Tasks'\n")
        .expect("fake xclip should be written");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&fake_xclip)
            .expect("fake xclip metadata")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fake_xclip, permissions)
            .expect("fake xclip should be executable");
    }

    let old_path = std::env::var_os("PATH");
    let test_path = match old_path.as_ref() {
        Some(path) => {
            let mut paths = vec![temp_dir.clone()];
            paths.extend(std::env::split_paths(path));
            std::env::join_paths(paths).expect("test path should be valid")
        }
        None => temp_dir.clone().into_os_string(),
    };

    unsafe {
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("DISPLAY", ":0");
        std::env::set_var("PATH", test_path);
    }

    let result = read_clipboard_image(16 * 1024 * 1024);

    unsafe {
        match old_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
    let _ = std::fs::remove_file(fake_xclip);
    let _ = std::fs::remove_dir(temp_dir);

    assert_eq!(result, None);
}

#[test]
fn read_clipboard_image_rejects_wayland_xclip_fallback_text_for_image_target() {
    let _guard = env_lock().lock().unwrap();
    let temp_dir =
        std::env::temp_dir().join(format!("herdr-fake-wayland-xclip-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let fake_wl_paste = temp_dir.join("wl-paste");
    let fake_xclip = temp_dir.join("xclip");
    std::fs::write(&fake_wl_paste, "#!/bin/sh\nexit 1\n").expect("fake wl-paste should be written");
    std::fs::write(&fake_xclip, "#!/bin/sh\nprintf '# Tasks'\n")
        .expect("fake xclip should be written");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        for command in [&fake_wl_paste, &fake_xclip] {
            let mut permissions = std::fs::metadata(command)
                .expect("fake clipboard command metadata")
                .permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(command, permissions)
                .expect("fake clipboard command should be executable");
        }
    }

    let old_path = std::env::var_os("PATH");
    let test_path = match old_path.as_ref() {
        Some(path) => {
            let mut paths = vec![temp_dir.clone()];
            paths.extend(std::env::split_paths(path));
            std::env::join_paths(paths).expect("test path should be valid")
        }
        None => temp_dir.clone().into_os_string(),
    };

    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
        std::env::set_var("DISPLAY", ":0");
        std::env::set_var("PATH", test_path);
    }

    let result = read_clipboard_image(16 * 1024 * 1024);

    unsafe {
        match old_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
    let _ = std::fs::remove_file(fake_wl_paste);
    let _ = std::fs::remove_file(fake_xclip);
    let _ = std::fs::remove_dir(temp_dir);

    assert_eq!(result, None);
}

#[test]
fn read_validated_clipboard_image_accepts_real_png_payload() {
    assert_eq!(
        read_validated_clipboard_image(
            "sh",
            &["-c", "printf '\\211PNG\\r\\n\\032\\nrest-of-image'"],
            "png",
            16 * 1024 * 1024,
        ),
        Some(ClipboardImage {
            bytes: b"\x89PNG\r\n\x1a\nrest-of-image".to_vec(),
            extension: "png",
        })
    );
}

#[test]
fn read_wsl_clipboard_image_accepts_png_from_windows_command() {
    assert_eq!(
        read_wsl_clipboard_image_with_command(|program| {
            assert_eq!(program, "powershell.exe");
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg("printf '\\211PNG\\r\\n\\032\\nrest-of-image'");
            command
        }, 16 * 1024 * 1024),
        Some(ClipboardImage {
            bytes: b"\x89PNG\r\n\x1a\nrest-of-image".to_vec(),
            extension: "png",
        })
    );
}

#[test]
fn image_signatures_match_only_their_format() {
    assert!(bytes_match_image_signature("png", b"\x89PNG\r\n\x1a\n..."));
    assert!(bytes_match_image_signature(
        "jpg",
        &[0xFF, 0xD8, 0xFF, 0xE0]
    ));
    assert!(bytes_match_image_signature("gif", b"GIF87a..."));
    assert!(bytes_match_image_signature("gif", b"GIF89a..."));
    assert!(bytes_match_image_signature(
        "webp",
        b"RIFF\x10\x00\x00\x00WEBPVP8 "
    ));

    let mut bmp = vec![0u8; 26];
    bmp[..2].copy_from_slice(b"BM");
    bmp[10] = 26;
    assert!(bytes_match_image_signature("bmp", &bmp));

    assert!(!bytes_match_image_signature("png", b"# Tasks"));
    assert!(!bytes_match_image_signature("jpg", b"plain clipboard text"));
    assert!(!bytes_match_image_signature("gif", b""));
    assert!(!bytes_match_image_signature("webp", b"RIFF but not webp"));
    assert!(!bytes_match_image_signature("bmp", b"\x89PNG\r\n\x1a\n"));
    assert!(!bytes_match_image_signature(
        "bmp",
        b"BM text is not a bitmap"
    ));
    assert!(!bytes_match_image_signature("svg", b"<svg></svg>"));
}

#[test]
fn desktop_notification_separates_option_like_titles() {
    let _guard = env_lock().lock().unwrap();
    unsafe {
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("DISPLAY", ":0");
    }

    let path = std::env::temp_dir().join(format!("herdr-notify-send-args-{}", std::process::id()));
    let script = "printf '%s\\n' \"$@\" > \"$HERDR_NOTIFY_ARGS\"";
    let shown = show_desktop_notification_with_command("-danger", Some("body"), |_| {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .arg("notify-send")
            .env("HERDR_NOTIFY_ARGS", &path);
        cmd
    })
    .expect("notification command should run");

    assert!(shown);
    let args = std::fs::read_to_string(&path).expect("args file");
    let _ = std::fs::remove_file(&path);
    assert_eq!(args, "--\n-danger\nbody\n");
}

#[test]
fn scrollback_editor_argv_preserves_unix_editor_shell_semantics() {
    let path = std::path::Path::new("/tmp/herdr scrollback.txt");
    let argv = scrollback_editor_argv(path).unwrap();

    assert_eq!(argv[0], "/bin/sh");
    assert_eq!(argv[1], "-c");
    assert!(argv[2].contains("EDITOR:-vi"));
    assert!(argv[2].contains("/tmp/herdr scrollback.txt"));
}
