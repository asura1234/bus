//! Keep the running Bus CLI available to every pane and provider child.
use portable_pty::CommandBuilder;
use std::{io, path::Path};

pub(super) fn apply_bus_cli_env(cmd: &mut CommandBuilder, executable: &Path) -> io::Result<()> {
    let directory = executable.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Bus executable has no directory",
        )
    })?;
    // Apply after launch overrides so every provider inherits this Bus CLI,
    // including dev builds whose target/debug directory is not on server PATH.
    let inherited = cmd.get_env("PATH").map(std::ffi::OsStr::to_os_string);
    let paths = std::iter::once(directory.to_path_buf()).chain(
        inherited
            .iter()
            .flat_map(std::env::split_paths)
            .filter(|path| path != directory),
    );
    let path = std::env::join_paths(paths)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    cmd.env("PATH", path);
    cmd.env("HERDR_BIN_PATH", executable);
    Ok(())
}
