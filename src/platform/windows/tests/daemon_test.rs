use super::*;

#[test]
fn windows_environment_keys_use_unicode_case_insensitive_ordering() {
    assert_eq!(
        super::super::windows_environment_key_cmp("hérdr", "HÉRDR"),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn windows_wmi_daemon_preserves_environment_and_working_directory() {
    if let Some(capture) = std::env::var_os(WMI_DAEMON_TEST_CHILD_ENV) {
        let cwd = std::env::current_dir().expect("WMI daemon test working directory");
        fs::write(
            capture,
            format!(
                "{}\n{}\n{}",
                cwd.display(),
                unsafe { GetConsoleWindow() }.is_null(),
                !super::super::current_job_kills_processes_on_close()
                    .expect("inspect WMI daemon job")
            ),
        )
        .expect("write WMI daemon test capture");
        return;
    }

    let base = std::env::temp_dir().join(format!(
        "herdr-wmi-daemon-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    fs::create_dir_all(&base).unwrap();
    let capture = base.join("capture.txt");
    let test_exe = std::env::current_exe().expect("resolve test executable");
    let mut child = Command::new(test_exe);
    child
        .arg("windows_wmi_daemon_preserves_environment_and_working_directory")
        .current_dir(&base)
        .env(WMI_DAEMON_TEST_CHILD_ENV, &capture)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let pid = super::super::launch_server_daemon_with_wmi(&child)
        .expect("launch detached process through WMI");
    assert_ne!(pid, 0, "WMI returned an invalid process id");

    let expected = format!("{}\ntrue\ntrue", base.display());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::read_to_string(&capture).is_ok_and(|captured| captured == expected) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "WMI daemon child did not write the expected capture"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_dir_all(base);
}

#[test]
fn windows_background_and_server_daemon_commands_do_not_have_consoles() {
    if let Some(mode) = std::env::var_os(CONSOLE_TEST_CHILD_ENV) {
        assert!(
            unsafe { GetConsoleWindow() }.is_null(),
            "{} child opened or inherited a console window",
            mode.to_string_lossy()
        );
        let parent_pid = std::env::var(CONSOLE_TEST_PARENT_PID_ENV)
            .expect("console test parent pid")
            .parse::<u32>()
            .expect("numeric console test parent pid");
        assert!(
            !console_process_ids().contains(&parent_pid),
            "{} child inherited the parent console",
            mode.to_string_lossy()
        );
        return;
    }

    let allocated_console = if console_process_ids().is_empty() {
        assert_ne!(unsafe { AllocConsole() }, 0, "allocate test console");
        true
    } else {
        false
    };

    let parent_pid = std::process::id().to_string();
    let test_exe = std::env::current_exe().expect("resolve test executable");
    type CommandConfigurator = fn(&mut Command);
    let configurations: [(&str, CommandConfigurator); 2] = [
        (
            "background",
            super::super::configure_background_command_platform,
        ),
        ("server daemon", super::super::detach_server_daemon_command),
    ];
    for (mode, configure) in configurations {
        let mut child = Command::new(&test_exe);
        child
            .arg("windows_background_and_server_daemon_commands_do_not_have_consoles")
            .env(CONSOLE_TEST_CHILD_ENV, mode)
            .env(CONSOLE_TEST_PARENT_PID_ENV, &parent_pid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure(&mut child);

        let status = child.status().expect("spawn console isolation test child");
        assert!(
            status.success(),
            "{mode} child opened or inherited a console"
        );
    }

    if allocated_console {
        unsafe {
            FreeConsole();
        }
    }
}
