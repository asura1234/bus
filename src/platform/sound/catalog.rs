use std::path::Path;
use std::path::PathBuf;

/// The name Bus shows for its own built-in ding.
pub const DEFAULT_SOUND_NAME: &str = "Default";

/// Audio files the platform players can decode, by extension.
pub(super) const SYSTEM_SOUND_EXTENSIONS: &[&str] =
    &["aiff", "aif", "caf", "wav", "oga", "ogg", "mp3"];

/// Sound themes nest by theme and variant; stop before unrelated trees.
pub(super) const SYSTEM_SOUND_DEPTH: usize = 3;

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
pub(super) fn system_sound_dirs() -> Vec<PathBuf> {
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
pub(super) fn system_sound_dirs() -> Vec<PathBuf> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    vec![PathBuf::from(root).join("Media")]
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn system_sound_dirs() -> Vec<PathBuf> {
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

pub(super) fn collect_sounds(dir: &Path, depth: usize, found: &mut Vec<SystemSound>) {
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
