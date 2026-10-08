use std::{
    fs,
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::System::Console::{
    AllocConsole, FreeConsole, GetConsoleProcessList, GetConsoleWindow,
};

const CONSOLE_TEST_CHILD_ENV: &str = "HERDR_TEST_CONSOLE_CHILD_MODE";
const CONSOLE_TEST_PARENT_PID_ENV: &str = "HERDR_TEST_CONSOLE_PARENT_PID";
const WMI_DAEMON_TEST_CHILD_ENV: &str = "HERDR_TEST_WMI_DAEMON_CHILD";

fn console_process_ids() -> Vec<u32> {
    let mut process_ids = vec![0; 8];
    loop {
        let count =
            unsafe { GetConsoleProcessList(process_ids.as_mut_ptr(), process_ids.len() as u32) }
                as usize;
        if count == 0 {
            return Vec::new();
        }
        if count <= process_ids.len() {
            process_ids.truncate(count);
            return process_ids;
        }
        process_ids.resize(count, 0);
    }
}

fn test_entry(pid: u32, parent_pid: u32, name: &str, argv: &[&str]) -> super::WindowsProcessEntry {
    test_entry_with_creation_time(pid, parent_pid, name, argv, None)
}

fn test_entry_without_cmdline(
    pid: u32,
    parent_pid: u32,
    name: &str,
    creation_time: u64,
) -> super::WindowsProcessEntry {
    let command = super::OnceLock::new();
    command
        .set(super::WindowsProcessCommand::from_cmdline(
            name,
            Some(creation_time),
            None,
        ))
        .unwrap();
    super::WindowsProcessEntry {
        pid,
        parent_pid,
        name: name.to_string(),
        command,
    }
}

fn test_entry_with_creation_time(
    pid: u32,
    parent_pid: u32,
    name: &str,
    argv: &[&str],
    creation_time: Option<u64>,
) -> super::WindowsProcessEntry {
    let command = super::OnceLock::new();
    command
        .set(super::WindowsProcessCommand {
            creation_time,
            argv0: argv.first().map(|value| (*value).to_string()),
            argv: Some(argv.iter().map(|value| (*value).to_string()).collect()),
            cmdline: Some(argv.join(" ")),
        })
        .unwrap();
    super::WindowsProcessEntry {
        pid,
        parent_pid,
        name: name.to_string(),
        command,
    }
}

#[path = "daemon_test.rs"]
mod daemon;

#[path = "process_test.rs"]
mod process;

#[path = "foreground_cache_test.rs"]
mod foreground_cache;

#[path = "input_test.rs"]
mod input;

#[test]
fn private_remote_directory_supports_long_paths() {
    let base = std::env::temp_dir().join(format!(
        "herdr-private-remote-dir-test-{}",
        std::process::id()
    ));
    fs::create_dir_all(&base).expect("create test base");
    let private = base.join("x".repeat(240));

    super::create_remote_private_dir(&private).expect("create private long-path directory");
    fs::write(private.join("probe"), b"ok").expect("write inherited private file");

    fs::remove_dir_all(base).expect("remove test directory");
}

#[test]
fn windows_notification_text_is_null_terminated_and_unicode_safe() {
    let mut destination = [u16::MAX; 6];
    super::copy_wide_truncated(&mut destination, "abc😀def");

    assert_eq!(String::from_utf16(&destination[..5]).unwrap(), "abc😀");
    assert_eq!(destination[5], 0);
}

#[test]
fn powershell_agent_command_omits_argument_list_when_no_arguments_are_passed() {
    let argv = vec!["opencode".into()];

    assert_eq!(
        super::interactive_shell_command(&argv, "powershell.exe").as_deref(),
        Some("& opencode")
    );
}

#[test]
fn cmd_agent_command_encodes_edge_arguments_without_cmd_expansion() {
    use base64::Engine as _;

    assert_eq!(super::super::quote_powershell_arg("@options"), "'@options'");
    let argv = vec![
        "pi".into(),
        String::new(),
        "two words".into(),
        "100%".into(),
        "wow!".into(),
        "a'b".into(),
        "--model".into(),
    ];
    let command = super::interactive_shell_command(&argv, "cmd.exe").unwrap();
    let encoded = command.split_whitespace().last().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    let utf16 = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    assert_eq!(
        String::from_utf16(&utf16).unwrap(),
        "if((Get-Command pi -ErrorAction SilentlyContinue).CommandType -eq 'ExternalScript'){& pi '' 'two words' '100%' 'wow!' 'a''b' '--model'}else{Start-Process -FilePath pi -ArgumentList '\"\" \"two words\" 100% wow! a''b --model' -NoNewWindow -Wait}"
    );
}

#[test]
fn windows_shells_round_trip_agent_arguments_through_a_real_command() {
    let _lock = crate::utils::test_env::env_lock();
    let base = std::env::temp_dir().join(format!(
        "herdr-agent-argv-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    fs::create_dir_all(&base).unwrap();
    let helper = base.join("pi.cmd");
    fs::write(
        &helper,
        "@echo off\r\n>\"%HERDR_ARGV_CAPTURE%\" (\r\necho(%~1\r\necho(%~2\r\necho(%~3\r\necho(%~4\r\necho(%~5\r\necho(%~6\r\necho(%~7\r\n)\r\n",
    )
    .unwrap();
    let argv = vec![
        "pi".into(),
        String::new(),
        "two words".into(),
        "100%".into(),
        "wow!".into(),
        "a'b".into(),
        "@options".into(),
        "--model".into(),
    ];
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{};{}", base.display(), inherited_path.to_string_lossy());
    let run_command = |shell: &str, command: &str, capture: &std::path::Path| {
        let mut process = if shell == "cmd.exe" {
            let mut process = Command::new("cmd.exe");
            process.args(["/d", "/c", command]);
            process
        } else {
            let mut process = Command::new("powershell.exe");
            process.args(["-NoLogo", "-NoProfile", "-Command", command]);
            process
        };
        process
            .env("PATH", &path)
            .env("HERDR_ARGV_CAPTURE", capture)
            .env("PSExecutionPolicyPreference", "Bypass")
            .status()
            .unwrap()
    };

    for shell in ["powershell.exe", "cmd.exe"] {
        let no_args_capture = base.join(format!("{shell}-no-args.txt"));
        let no_args_command = super::interactive_shell_command(&["pi".into()], shell).unwrap();
        let status = run_command(shell, &no_args_command, &no_args_capture);
        assert!(status.success(), "{shell} argument-free command failed");
        assert_eq!(
            fs::read_to_string(no_args_capture)
                .unwrap()
                .replace("\r\n", "\n"),
            "\n\n\n\n\n\n\n"
        );

        let capture = base.join(format!("{shell}.txt"));
        let command = super::interactive_shell_command(&argv, shell).unwrap();
        let status = run_command(shell, &command, &capture);
        assert!(status.success(), "{shell} command failed");
        assert_eq!(
            fs::read_to_string(capture).unwrap().replace("\r\n", "\n"),
            "\ntwo words\n100%\nwow!\na'b\n@options\n--model\n"
        );
    }

    fs::remove_file(helper).unwrap();
    fs::write(
        base.join("pi.ps1"),
        "Set-Content -LiteralPath $env:HERDR_ARGV_CAPTURE -Value @(\"$($args[0])\", \"$($args[1])\", \"$($args[2])\", \"$($args[3])\", \"$($args[4])\", \"$($args[5])\", \"$($args[6])\")\r\n",
    )
    .unwrap();
    for shell in ["powershell.exe", "cmd.exe"] {
        let capture = base.join(format!("{shell}-ps1.txt"));
        let command = super::interactive_shell_command(&argv, shell).unwrap();
        let status = run_command(shell, &command, &capture);
        assert!(status.success(), "{shell} PowerShell script command failed");
        assert_eq!(
            fs::read_to_string(capture).unwrap().replace("\r\n", "\n"),
            "\ntwo words\n100%\nwow!\na'b\n@options\n--model\n"
        );
    }

    let _ = fs::remove_dir_all(base);
}

#[test]
fn windows_shell_is_available_only_without_descendants() {
    let shell_only = super::ProcessSnapshot::new(vec![test_entry(
        10,
        1,
        "powershell.exe",
        &["powershell.exe"],
    )]);
    assert_eq!(
        super::available_pane_shell_from_snapshot(10, &shell_only).as_deref(),
        Some("powershell.exe")
    );

    let busy = super::ProcessSnapshot::new(vec![
        test_entry(10, 1, "powershell.exe", &["powershell.exe"]),
        test_entry(20, 10, "git.exe", &["git.exe", "status"]),
    ]);
    assert_eq!(super::available_pane_shell_from_snapshot(10, &busy), None);

    let replaced = super::ProcessSnapshot::new(vec![test_entry(10, 1, "vim.exe", &["vim.exe"])]);
    assert_eq!(
        super::available_pane_shell_from_snapshot(10, &replaced),
        None
    );
}

#[test]
fn scrollback_editor_argv_uses_editor_env_and_appends_path() {
    let path = std::path::Path::new(r"C:\Users\User\AppData\Local\Temp\herdr scrollback.txt");
    let argv = super::scrollback_editor_argv_with_env(
        path,
        Some(r#""C:\Program Files\Microsoft VS Code\Code.exe" --wait"#),
    )
    .unwrap();

    assert_eq!(argv[0], r"C:\Program Files\Microsoft VS Code\Code.exe");
    assert_eq!(argv[1], "--wait");
    assert_eq!(argv[2], path.display().to_string());
}

#[test]
fn scrollback_editor_argv_falls_back_to_notepad() {
    let path = std::path::Path::new(r"C:\Temp\herdr-scrollback.txt");
    let argv = super::scrollback_editor_argv_with_env(path, None).unwrap();

    assert_eq!(
        argv,
        vec!["notepad.exe".to_string(), path.display().to_string()]
    );
}
