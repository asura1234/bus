//! Resolve the physical Bus image independently of CLI alias entry points.
use std::{
    io,
    path::{Path, PathBuf},
};

/// An alias is a command entry point, not the image to use for another launch.
pub(crate) fn running_executable() -> io::Result<PathBuf> {
    resolve_running_executable(&std::env::current_exe()?)
}

pub(crate) fn resolve_running_executable(image: &Path) -> io::Result<PathBuf> {
    let image = match image.canonicalize() {
        Ok(image) => image,
        // Linux can report a removed image after an in-place dev rebuild.
        // Preserve the old nonfatal current_exe behavior for later pane launches.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(image.to_path_buf()),
        Err(error) => return Err(error),
    };
    #[cfg(windows)]
    if let Some(origin) = alias_origin(&image)? {
        return Ok(origin);
    }
    Ok(image)
}

/// The Windows alias retains a small, stable bootstrap entry point. Running
/// the real image preserves source discovery, build identity and restart paths.
#[cfg(windows)]
pub(crate) fn forward_cli_alias() -> io::Result<Option<std::process::ExitStatus>> {
    let image = std::env::current_exe()?.canonicalize()?;
    let Some(origin) = alias_origin(&image)? else {
        return Ok(None);
    };
    std::process::Command::new(origin)
        .args(std::env::args_os().skip(1))
        .status()
        .map(Some)
}

#[cfg(windows)]
fn alias_origin(image: &Path) -> io::Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    let Some(directory) = image.parent() else {
        return Ok(None);
    };
    let named = |path: &Path, name: &str| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(name))
    };
    if !named(directory, "cli-bin") || !named(image, "bus.exe") {
        return Ok(None);
    }
    let bytes = match std::fs::read(directory.join(".bus-origin")) {
        Ok(bytes) => bytes,
        // A normal installation may happen to use this directory layout.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let chunks = bytes.chunks_exact(2);
    if bytes.is_empty() || !chunks.remainder().is_empty() {
        return Err(io::Error::other("Bus CLI origin is invalid"));
    }
    let units = chunks
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let origin = PathBuf::from(std::ffi::OsString::from_wide(&units));
    if !origin.is_absolute() {
        return Err(io::Error::other("Bus CLI origin must be absolute"));
    }
    let origin = origin.canonicalize()?;
    if origin.starts_with(directory) {
        return Err(io::Error::other(
            "Bus CLI origin points into its alias directory",
        ));
    }
    Ok(Some(origin))
}
