//! Notification sounds and Bus room dings.
//!
//! Embeds mp3 files in the binary and plays them via system audio tools.
//! Uses afplay (macOS), Windows MediaPlayer, or decoder-capable Linux audio
//! players — no Rust audio dependencies.

use std::io::Write;
#[cfg(not(any(windows, target_os = "macos")))]
use std::io::{Read, Result as IoResult};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(any(windows, target_os = "macos")))]
use std::time::{Duration, Instant};

use tracing::warn;

const DISABLE_SOUND_ENV: &str = "HERDR_DISABLE_SOUND";
#[cfg(any(windows, test))]
const WINDOWS_SOUND_PATH_ENV: &str = "HERDR_SOUND_PATH";
#[cfg(not(any(windows, target_os = "macos")))]
const AUDIO_PLAYER_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(not(any(windows, target_os = "macos")))]
const AUDIO_PLAYER_POLL_INTERVAL: Duration = Duration::from_millis(25);

static SOUND_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static SOUND_DONE: &[u8] = include_bytes!("../assets/sounds/done.mp3");
static SOUND_REQUEST: &[u8] = include_bytes!("../assets/sounds/request.mp3");

/// Which notification sound to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// Something finished.
    Done,
    /// Something needs attention.
    Request,
}

/// Play a notification sound in a background thread.
/// Silently does nothing if no audio player is available.
pub fn play(sound: Sound, config: &crate::config::SoundConfig) {
    if sound_playback_disabled_by_env() {
        return;
    }

    let custom_path = config.path_for(sound);
    std::thread::spawn(move || {
        if let Some(path) = custom_path {
            match play_file(&path) {
                Ok(()) => return,
                Err(err) => {
                    warn!(path = %path.display(), sound = ?sound, err = %err, "custom sound playback failed, falling back to built-in sound")
                }
            }
        }

        let data = match sound {
            Sound::Done => SOUND_DONE,
            Sound::Request => SOUND_REQUEST,
        };

        if let Err(err) = play_bytes(data) {
            warn!(sound = ?sound, err = %err, "sound playback failed");
        }
    });
}

fn sound_playback_disabled_by_env() -> bool {
    std::env::var_os(DISABLE_SOUND_ENV).is_some() || std::env::var_os("NEXTEST").is_some()
}

fn play_file(path: &Path) -> Result<(), String> {
    match run_player(path) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(playback_error(&output)),
        Err(err) => Err(err),
    }
}

fn play_bytes(data: &[u8]) -> Result<(), String> {
    // Write to a temp file because the supported audio players need a file path.
    let tmp = temp_sound_path();
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())?;
    drop(file);

    let result = play_file(&tmp);

    let _ = std::fs::remove_file(&tmp);

    result
}

fn playback_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("player exited with {}", output.status)
    } else {
        format!("player exited with {}: {stderr}", output.status)
    }
}

fn temp_sound_path() -> PathBuf {
    let id = SOUND_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("herdr-sound-{}-{id}.mp3", std::process::id()))
}

#[cfg(windows)]
fn run_player(path: &Path) -> Result<Output, String> {
    run_windows_player(path)
}

#[cfg(target_os = "macos")]
fn run_player(path: &Path) -> Result<Output, String> {
    Command::new("afplay")
        .arg(path)
        .output()
        .map_err(|e| format!("no audio player available: {e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_player(path: &Path) -> Result<Output, String> {
    run_linux_player(path)
}

#[cfg(any(windows, test))]
fn windows_media_player_script() -> &'static str {
    r#"
$ErrorActionPreference = 'Stop'
$Path = [Environment]::GetEnvironmentVariable('HERDR_SOUND_PATH', 'Process')
if ([string]::IsNullOrWhiteSpace($Path)) { throw 'HERDR_SOUND_PATH is not set' }
Add-Type -AssemblyName PresentationCore
Add-Type -AssemblyName WindowsBase
$resolved = (Resolve-Path -LiteralPath $Path).ProviderPath
$script:player = [System.Windows.Media.MediaPlayer]::new()
$script:frame = [System.Windows.Threading.DispatcherFrame]::new()
$script:timer = [System.Windows.Threading.DispatcherTimer]::new()
$script:timer.Interval = [TimeSpan]::FromSeconds(15)
$script:failed = $null
$script:timedOut = $false
$script:player.add_MediaOpened({ $script:player.Play() })
$script:player.add_MediaEnded({ $script:frame.Continue = $false })
$script:player.add_MediaFailed({
    param($sender, $eventArgs)
    $script:failed = $eventArgs.ErrorException
    $script:frame.Continue = $false
})
$script:timer.add_Tick({
    $script:timedOut = $true
    $script:frame.Continue = $false
})
try {
    $script:player.Open([Uri]::new($resolved))
    $script:timer.Start()
    [System.Windows.Threading.Dispatcher]::PushFrame($script:frame)
} finally {
    $script:timer.Stop()
    $script:player.Close()
}
if ($script:failed) { throw "sound media failed: $($script:failed.Message)" }
if ($script:timedOut) { throw 'sound playback timed out' }
"#
}

#[cfg(any(windows, test))]
fn windows_player_command(path: &Path) -> Command {
    let mut command = crate::noninteractive_process::command("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            windows_media_player_script(),
        ])
        .env(WINDOWS_SOUND_PATH_ENV, path);
    command
}

#[cfg(windows)]
fn run_windows_player(path: &Path) -> Result<Output, String> {
    windows_player_command(path)
        .output()
        .map_err(|e| format!("Windows MediaPlayer playback failed: {e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
#[derive(Debug, Clone, Copy)]
struct AudioPlayer {
    program: &'static str,
    args: &'static [&'static str],
}

#[cfg(not(any(windows, target_os = "macos")))]
impl AudioPlayer {
    fn output(self, path: &Path) -> std::io::Result<Output> {
        self.output_with_timeout(path, AUDIO_PLAYER_TIMEOUT)
    }

    fn output_with_timeout(self, path: &Path, timeout: Duration) -> std::io::Result<Output> {
        let mut child = Command::new(self.program)
            .args(self.args)
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let Some(stdout) = child.stdout.take() else {
            terminate_and_reap(&mut child)?;
            return Err(std::io::Error::other("audio player stdout was not piped"));
        };
        let Some(stderr) = child.stderr.take() else {
            terminate_and_reap(&mut child)?;
            return Err(std::io::Error::other("audio player stderr was not piped"));
        };
        let stdout_reader = read_output(stdout);
        let stderr_reader = read_output(stderr);
        let deadline = Instant::now() + timeout;

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let (stdout, stderr) = finish_output(stdout_reader, stderr_reader)?;
                    return Ok(Output {
                        status,
                        stdout,
                        stderr,
                    });
                }
                Ok(None) => {}
                Err(wait_err) => {
                    let cleanup_result = terminate_and_reap(&mut child);
                    let _ = finish_output(stdout_reader, stderr_reader);
                    cleanup_result?;
                    return Err(wait_err);
                }
            }

            let now = Instant::now();
            if now >= deadline {
                let cleanup_result = terminate_and_reap(&mut child);
                let _ = finish_output(stdout_reader, stderr_reader);
                cleanup_result?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{} playback timed out after {timeout:?}", self.program),
                ));
            }

            std::thread::sleep((deadline - now).min(AUDIO_PLAYER_POLL_INTERVAL));
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn read_output<R>(mut reader: R) -> std::thread::JoinHandle<IoResult<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        Ok(output)
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn finish_output(
    stdout_reader: std::thread::JoinHandle<IoResult<Vec<u8>>>,
    stderr_reader: std::thread::JoinHandle<IoResult<Vec<u8>>>,
) -> IoResult<(Vec<u8>, Vec<u8>)> {
    let stdout = stdout_reader
        .join()
        .map_err(|_| std::io::Error::other("audio player stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("audio player stderr reader panicked"))??;
    Ok((stdout, stderr))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn terminate_and_reap(child: &mut std::process::Child) -> std::io::Result<()> {
    if let Err(kill_err) = child.kill() {
        if child.try_wait()?.is_none() {
            return Err(kill_err);
        }
    }
    child.wait().map(|_| ())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn linux_audio_players() -> &'static [AudioPlayer] {
    // Do not add bare aplay here. It does not decode MP3 and plays MP3 bytes as raw PCM.
    &[
        AudioPlayer {
            program: "paplay",
            args: &[],
        },
        AudioPlayer {
            program: "pw-play",
            args: &[],
        },
        AudioPlayer {
            program: "ffplay",
            args: &["-nodisp", "-autoexit", "-loglevel", "quiet"],
        },
        AudioPlayer {
            program: "mpg123",
            args: &["-q"],
        },
        AudioPlayer {
            program: "mpv",
            args: &["--no-video", "--really-quiet"],
        },
    ]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_linux_player(path: &Path) -> Result<Output, String> {
    let mut errors = Vec::new();

    for player in linux_audio_players() {
        match player.output(path) {
            Ok(output) if output.status.success() => return Ok(output),
            Ok(output) => errors.push(player_error(*player, &output)),
            Err(err) => errors.push(format!("{} failed: {err}", player.program)),
        }
    }

    Err(format!(
        "no mp3-capable audio player available: {}",
        errors.join("; ")
    ))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn player_error(player: AudioPlayer, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();

    if stderr.is_empty() {
        format!("{} exited with {}", player.program, output.status)
    } else {
        format!("{} exited with {}: {stderr}", player.program, output.status)
    }
}

/// The name Bus shows for its own built-in ding.
pub const DEFAULT_SOUND_NAME: &str = "Default";
/// Audio files the platform players can decode, by extension.
const SYSTEM_SOUND_EXTENSIONS: &[&str] = &["aiff", "aif", "caf", "wav", "oga", "ogg", "mp3"];
/// Sound themes nest by theme and variant; stop before unrelated trees.
const SYSTEM_SOUND_DEPTH: usize = 3;

/// A sound file installed with the operating system, named by its file stem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemSound {
    pub name: String,
    pub path: PathBuf,
}

/// The operating system's own notification sounds, sorted by name.
pub fn system_sounds() -> Vec<SystemSound> {
    list_sounds(&system_sound_dirs())
}

#[cfg(target_os = "macos")]
fn system_sound_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/System/Library/Sounds"),
        PathBuf::from("/Library/Sounds"),
    ];
    if let Some(home) = std::env::home_dir() {
        dirs.push(home.join("Library/Sounds"));
    }
    dirs
}

#[cfg(windows)]
fn system_sound_dirs() -> Vec<PathBuf> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    vec![PathBuf::from(root).join("Media")]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_sound_dirs() -> Vec<PathBuf> {
    // The freedesktop theme first, so its names win over other themes' copies.
    vec![
        PathBuf::from("/usr/share/sounds/freedesktop/stereo"),
        PathBuf::from("/usr/share/sounds"),
    ]
}

/// Sound files under `dirs`, one per name (earlier directories win), sorted
/// case-insensitively. Missing directories are skipped.
pub(crate) fn list_sounds(dirs: &[PathBuf]) -> Vec<SystemSound> {
    let mut found = Vec::new();
    for dir in dirs {
        collect_sounds(dir, SYSTEM_SOUND_DEPTH, &mut found);
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|sound: &SystemSound| {
        sound.name != DEFAULT_SOUND_NAME && seen.insert(sound.name.to_lowercase())
    });
    found.sort_by_key(|sound| sound.name.to_lowercase());
    found
}

fn collect_sounds(dir: &Path, depth: usize, found: &mut Vec<SystemSound>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if depth > 1 {
                collect_sounds(&path, depth - 1, found);
            }
            continue;
        }
        let is_sound = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                SYSTEM_SOUND_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
            });
        let name = path.file_stem().and_then(|stem| stem.to_str());
        if let (true, Some(name)) = (is_sound, name) {
            found.push(SystemSound {
                name: name.to_owned(),
                path: path.clone(),
            });
        }
    }
}

/// The sound named `name`, ignoring case.
pub fn find_sound<'a>(sounds: &'a [SystemSound], name: &str) -> Option<&'a SystemSound> {
    sounds
        .iter()
        .find(|sound| sound.name.eq_ignore_ascii_case(name))
}

/// Plays a room's sound in a background thread: the named system sound, or
/// the built-in ding when `name` is None, no longer installed, or unplayable.
pub fn play_named(name: Option<&str>, config: &crate::config::SoundConfig) {
    if sound_playback_disabled_by_env() {
        return;
    }
    let Some(path) = resolve_sound(name, &system_sounds()) else {
        play(Sound::Done, config);
        return;
    };
    let config = config.clone();
    std::thread::spawn(move || {
        if let Err(err) = play_file(&path) {
            warn!(path = %path.display(), err = %err, "system sound playback failed, playing the default ding");
            play(Sound::Done, &config);
        }
    });
}

/// The file to play for a room's sound name, or None for the default ding:
/// no name chosen, or that sound is no longer installed.
fn resolve_sound(name: Option<&str>, sounds: &[SystemSound]) -> Option<PathBuf> {
    let name = name?;
    let sound = find_sound(sounds, name);
    if sound.is_none() {
        warn!(name, "system sound not found, playing the default ding");
    }
    sound.map(|sound| sound.path.clone())
}

#[cfg(test)]
mod tests {
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
}
