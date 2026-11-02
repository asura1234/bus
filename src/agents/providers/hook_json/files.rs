//! Hook configuration path policy over platform file mechanics.
#[cfg(unix)]
use crate::platform::fs;
pub(crate) use crate::platform::fs::{
    guarded_atomic_write as atomic_write, guarded_lock_file as lock, private_dir,
};
#[cfg(unix)]
use std::{io, path::Path};

/// Preserve a matching executable; publish replacements only after fchmod on
/// the exclusively created file. Neither reads nor permission changes follow
/// a bridge symlink, and an existing hardlink's other names stay untouched.
#[cfg(unix)]
pub(super) fn executable_bridge(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::{Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(mut file) => {
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Err(io::Error::other("Bus bridge is not a regular file"));
            }
            let mut current = Vec::new();
            file.read_to_end(&mut current)?;
            if current == bytes && metadata.permissions().mode() & 0o7777 == 0o700 {
                return Ok(());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::atomic_write_with(path, |file| {
        file.write_all(bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o700))
    })
    .map_err(|error| error.source)
}
