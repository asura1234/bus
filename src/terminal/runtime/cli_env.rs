//! Keep the running Bus CLI available to every pane and provider child.
use portable_pty::CommandBuilder;
use std::{io, path::Path};

pub(crate) fn prepare_bus_cli(data_dir: &Path, executable: &Path) -> io::Result<()> {
    let directory = data_dir.join("cli-bin");
    crate::platform::fs::private_dir(&directory)?;
    crate::platform::fs::refresh_executable_alias(
        &directory.join(format!("bus{}", std::env::consts::EXE_SUFFIX)),
        executable,
    )
}

pub(super) fn apply_bus_cli_env(
    cmd: &mut CommandBuilder,
    executable: &Path,
    data_dir: &Path,
) -> io::Result<()> {
    // Expose only bus. Its installation directory may also contain provider
    // commands or user tools whose PATH priority must remain unchanged.
    let directory = data_dir.join("cli-bin");
    let inherited = cmd.get_env("PATH").map(std::ffi::OsStr::to_os_string);
    let paths = std::iter::once(directory.to_path_buf()).chain(
        inherited
            .iter()
            .flat_map(std::env::split_paths)
            .filter(|path| path != &directory),
    );
    let path = std::env::join_paths(paths)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    cmd.env("PATH", path);
    cmd.env("HERDR_BIN_PATH", executable);
    Ok(())
}

pub(crate) fn apply_pane_base_env(cmd: &mut CommandBuilder) -> io::Result<()> {
    cmd.env(
        crate::protocol::api::SOCKET_PATH_ENV_VAR,
        crate::protocol::api::socket_path(),
    );
    apply_bus_cli_env(
        cmd,
        &std::env::current_exe()?,
        &crate::utils::paths::data_dir(),
    )?;
    cmd.env(
        crate::agents::providers::callback_entry::BUS_SERVER_PID_ENV_VAR,
        std::process::id().to_string(),
    );
    Ok(())
}
