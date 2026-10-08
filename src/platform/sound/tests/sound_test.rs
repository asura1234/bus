use super::*;

fn sound_tree(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bus-system-sounds-{label}-{}-{}",
        std::process::id(),
        SOUND_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
}

#[test]
fn system_sounds_list_audio_files_by_name_across_folders() {
    let root = sound_tree("list");
    let mac = root.join("System/Library/Sounds");
    let windows = root.join("Windows/Media");
    let linux = root.join("usr/share/sounds");
    for path in [
        mac.join("Glass.aiff"),
        mac.join("Basso.aiff"),
        windows.join("Windows Notify.wav"),
        windows.join("chimes.WAV"),
        linux.join("freedesktop/stereo/complete.oga"),
        linux.join("freedesktop/stereo/message.oga"),
        linux.join("freedesktop/index.theme"),
        mac.join("README.txt"),
    ] {
        touch(&path);
    }
    let sounds = list_sounds(&[mac.clone(), windows, linux, root.join("missing")]);
    let names: Vec<_> = sounds.iter().map(|sound| sound.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Basso",
            "chimes",
            "complete",
            "Glass",
            "message",
            "Windows Notify"
        ]
    );
    assert_eq!(
        find_sound(&sounds, "glass").map(|sound| sound.path.clone()),
        Some(mac.join("Glass.aiff"))
    );
    assert!(find_sound(&sounds, "Sosumi").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_or_unchosen_sounds_fall_back_to_the_default_ding() {
    let sounds = [SystemSound {
        name: "Glass".into(),
        path: PathBuf::from("/sounds/Glass.aiff"),
    }];
    assert_eq!(
        resolve_sound(Some("glass"), &sounds),
        Some(PathBuf::from("/sounds/Glass.aiff"))
    );
    assert_eq!(resolve_sound(Some("Sosumi"), &sounds), None);
    assert_eq!(resolve_sound(None, &sounds), None);
}

#[test]
fn earlier_sound_folders_win_duplicate_names_and_default_is_reserved() {
    let root = sound_tree("duplicates");
    let preferred = root.join("freedesktop/stereo");
    let other = root.join("other");
    touch(&preferred.join("bell.oga"));
    touch(&other.join("Bell.wav"));
    touch(&other.join("Default.wav"));
    touch(&other.join("a/b/c/too-deep.wav"));
    let sounds = list_sounds(&[preferred.clone(), other]);
    assert_eq!(
        sounds,
        [SystemSound {
            name: "bell".into(),
            path: preferred.join("bell.oga"),
        }]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn temp_sound_paths_are_unique() {
    assert_ne!(temp_sound_path(), temp_sound_path());
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn linux_audio_players_are_mp3_capable() {
    let programs: Vec<&str> = linux_audio_players()
        .iter()
        .map(|player| player.program)
        .collect();

    assert_eq!(programs, ["paplay", "pw-play", "ffplay", "mpg123", "mpv"]);
    assert!(!programs.contains(&"aplay"));
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn linux_audio_player_does_not_wait_forever() {
    let pid_path = temp_sound_path().with_extension("pid");
    let player = AudioPlayer {
        program: "sh",
        args: &[
            "-c",
            "printf '%s' \"$$\" > \"$1\"; exec sleep 2",
            "herdr-sound-timeout-test",
        ],
    };
    let result = player.output_with_timeout(&pid_path, Duration::from_millis(100));
    let pid = std::fs::read_to_string(&pid_path)
        .expect("hanging test player should record its process ID");
    let _ = std::fs::remove_file(pid_path);

    let err = result.expect_err("hanging audio player should time out");
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    let status = Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .expect("test should inspect the timed-out player PID");
    assert!(
        !status.success(),
        "timed-out audio player should be terminated and reaped"
    );
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn linux_audio_player_preserves_completed_output() {
    let player = AudioPlayer {
        program: "sh",
        args: &[
            "-c",
            "i=0; while [ \"$i\" -lt 8192 ]; do printf 0123456789abcdef; i=$((i + 1)); done; i=0; while [ \"$i\" -lt 8192 ]; do printf fedcba9876543210; i=$((i + 1)); done >&2; exit 7",
            "herdr-sound-output-test",
        ],
    };

    let output = player
        .output_with_timeout(Path::new("unused.mp3"), Duration::from_secs(5))
        .expect("completed audio player should return its output");

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout.len(), 131_072);
    assert_eq!(output.stderr.len(), 131_072);
    assert!(output.stdout.starts_with(b"0123456789abcdef"));
    assert!(output.stderr.starts_with(b"fedcba9876543210"));
}

#[test]
fn windows_media_player_uses_process_environment_and_dispatcher() {
    let script = windows_media_player_script();
    let path = Path::new(r"C:\sound dir\döne.mp3");
    let command = windows_player_command(path);
    let env_path = command.get_envs().find_map(|(key, value)| {
        (key == std::ffi::OsStr::new(WINDOWS_SOUND_PATH_ENV))
            .then_some(value)
            .flatten()
    });

    assert!(script.contains("GetEnvironmentVariable('HERDR_SOUND_PATH', 'Process')"));
    assert!(!script.contains("param([string]$Path)"));
    assert!(script.contains("Resolve-Path -LiteralPath $Path"));
    assert!(script.contains("Dispatcher]::PushFrame"));
    assert!(script.contains("add_MediaEnded"));
    assert!(script.contains("add_MediaFailed"));
    assert_eq!(env_path, Some(path.as_os_str()));
    assert!(!command.get_args().any(|arg| arg == path.as_os_str()));
}

#[cfg(windows)]
#[test]
fn windows_media_player_reports_invalid_media_without_waiting_for_timeout() {
    let _lock = crate::pane::env::env_lock();
    let path = temp_sound_path();
    std::fs::write(&path, b"not an mp3").unwrap();
    let output = run_windows_player(&path).unwrap();
    let _ = std::fs::remove_file(path);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("sound media failed"),
        "stderr should identify a MediaFailed error"
    );
}
