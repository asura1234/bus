//! Notification sounds: playback and the installed sound catalog.
mod catalog;
mod player;
pub use catalog::find_sound;
pub(crate) use catalog::list_sounds;
pub use catalog::system_sounds;
pub use catalog::SystemSound;
pub use catalog::DEFAULT_SOUND_NAME;
#[cfg(test)]
#[cfg(not(any(windows, target_os = "macos")))]
use player::linux_audio_players;
pub use player::play_named;
#[cfg(test)]
use player::resolve_sound;
#[cfg(test)]
#[cfg(windows)]
use player::run_windows_player;
#[cfg(test)]
use player::temp_sound_path;
#[cfg(test)]
#[cfg(any(windows, test))]
use player::windows_media_player_script;
#[cfg(test)]
#[cfg(any(windows, test))]
use player::windows_player_command;
#[cfg(test)]
#[cfg(not(any(windows, target_os = "macos")))]
use player::AudioPlayer;
#[cfg(test)]
use player::SOUND_TMP_COUNTER;
#[cfg(test)]
#[cfg(any(windows, test))]
use player::WINDOWS_SOUND_PATH_ENV;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(all(test, not(any(windows, target_os = "macos"))))]
use std::process::Command;
#[cfg(test)]
use std::sync::atomic::Ordering;
#[cfg(test)]
#[cfg(not(any(windows, target_os = "macos")))]
use std::time::Duration;

/// Which notification sound to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// Something finished.
    Done,
}

#[cfg(test)]
#[path = "tests/sound_test.rs"]
mod tests;
