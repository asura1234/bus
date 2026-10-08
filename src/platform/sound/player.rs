use crate::platform::sound::catalog::find_sound;
use crate::platform::sound::catalog::system_sounds;
use crate::platform::sound::catalog::SystemSound;
use crate::platform::sound::Sound;
#[cfg(not(any(windows, target_os = "macos")))]
use std::io::Read;
#[cfg(not(any(windows, target_os = "macos")))]
use std::io::Result as IoResult;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
#[cfg(not(any(windows, target_os = "macos")))]
use std::time::Duration;
#[cfg(not(any(windows, target_os = "macos")))]
use std::time::Instant;
use tracing::warn;

pub(super) const DISABLE_SOUND_ENV: &str = "HERDR_DISABLE_SOUND";

#[cfg(any(windows, test))]
pub(super) const WINDOWS_SOUND_PATH_ENV: &str = "HERDR_SOUND_PATH";

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) const AUDIO_PLAYER_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) const AUDIO_PLAYER_POLL_INTERVAL: Duration = Duration::from_millis(25);

pub(super) static SOUND_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) static SOUND_DONE: &[u8] = include_bytes!("../../../assets/sounds/done.mp3");

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
        };

        if let Err(err) = play_bytes(data) {
            warn!(sound = ?sound, err = %err, "sound playback failed");
        }
    });
}

pub(super) fn sound_playback_disabled_by_env() -> bool {
    std::env::var_os(DISABLE_SOUND_ENV).is_some() || std::env::var_os("NEXTEST").is_some()
}

pub(super) fn play_file(path: &Path) -> Result<(), String> {
    match run_player(path) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(playback_error(&output)),
        Err(err) => Err(err),
    }
}

pub(super) fn play_bytes(data: &[u8]) -> Result<(), String> {
    // Write to a temp file because the supported audio players need a file path.
    let tmp = temp_sound_path();
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())?;
    drop(file);

    let result = play_file(&tmp);

    let _ = std::fs::remove_file(&tmp);

    result
}

pub(super) fn playback_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("player exited with {}", output.status)
    } else {
        format!("player exited with {}: {stderr}", output.status)
    }
}

pub(super) fn temp_sound_path() -> PathBuf {
    let id = SOUND_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("herdr-sound-{}-{id}.mp3", std::process::id()))
}

#[cfg(windows)]
pub(super) fn run_player(path: &Path) -> Result<Output, String> {
    run_windows_player(path)
}

#[cfg(target_os = "macos")]
pub(super) fn run_player(path: &Path) -> Result<Output, String> {
    Command::new("afplay")
        .arg(path)
        .output()
        .map_err(|e| format!("no audio player available: {e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn run_player(path: &Path) -> Result<Output, String> {
    run_linux_player(path)
}

#[cfg(any(windows, test))]
pub(super) fn windows_media_player_script() -> &'static str {
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
pub(super) fn windows_player_command(path: &Path) -> Command {
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
pub(super) fn run_windows_player(path: &Path) -> Result<Output, String> {
    windows_player_command(path)
        .output()
        .map_err(|e| format!("Windows MediaPlayer playback failed: {e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
#[derive(Debug, Clone, Copy)]
pub(super) struct AudioPlayer {
    pub(super) program: &'static str,
    pub(super) args: &'static [&'static str],
}

#[cfg(not(any(windows, target_os = "macos")))]
impl AudioPlayer {
    pub(super) fn output(self, path: &Path) -> std::io::Result<Output> {
        self.output_with_timeout(path, AUDIO_PLAYER_TIMEOUT)
    }

    pub(super) fn output_with_timeout(
        self,
        path: &Path,
        timeout: Duration,
    ) -> std::io::Result<Output> {
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
pub(super) fn read_output<R>(mut reader: R) -> std::thread::JoinHandle<IoResult<Vec<u8>>>
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
pub(super) fn finish_output(
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
pub(super) fn terminate_and_reap(child: &mut std::process::Child) -> std::io::Result<()> {
    if let Err(kill_err) = child.kill() {
        if child.try_wait()?.is_none() {
            return Err(kill_err);
        }
    }
    child.wait().map(|_| ())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn linux_audio_players() -> &'static [AudioPlayer] {
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
pub(super) fn run_linux_player(path: &Path) -> Result<Output, String> {
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
pub(super) fn player_error(player: AudioPlayer, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();

    if stderr.is_empty() {
        format!("{} exited with {}", player.program, output.status)
    } else {
        format!("{} exited with {}: {stderr}", player.program, output.status)
    }
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
pub(super) fn resolve_sound(name: Option<&str>, sounds: &[SystemSound]) -> Option<PathBuf> {
    let name = name?;
    let sound = find_sound(sounds, name);
    if sound.is_none() {
        warn!(name, "system sound not found, playing the default ding");
    }
    sound.map(|sound| sound.path.clone())
}
