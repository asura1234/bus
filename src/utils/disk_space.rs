//! Free space on the volume that holds a data directory.
//!
//! The restart preflight and the messaging coordinator both use this so a
//! guarded test or dev-tools scratch fixture cannot disagree with the live measurement.
use std::path::Path;

pub(crate) fn available_bytes(data_dir: &Path) -> std::io::Result<u64> {
    // The launch guard validates the protected HOME before a test Bus boots.
    // Normal scratch launches and dev-tools sessions use only their own fixture.
    let synthetic = std::env::var_os("TEST_BUS_PROTECTED_HOME").is_some();
    #[cfg(feature = "dev-tools")]
    let synthetic = synthetic
        || std::env::var_os("BUS_TUI_OWNER").is_some()
        || std::env::var("BUS_TUI_FAKE_AGENT").as_deref() == Ok("1");
    if synthetic {
        return match std::fs::read_to_string(data_dir.join("disk-free-bytes")) {
            Ok(value) => value
                .trim()
                .parse()
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(u64::MAX),
            Err(error) => Err(error),
        };
    }
    volume_available_bytes(data_dir)
}

#[cfg(unix)]
fn volume_available_bytes(data_dir: &Path) -> std::io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(data_dir.as_os_str().as_bytes())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated, and statvfs initializes stats on success.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: statvfs returned success above. bavail excludes reserved blocks.
    let stats = unsafe { stats.assume_init() };
    Ok((u128::from(stats.f_bavail) * u128::from(stats.f_frsize)).min(u128::from(u64::MAX)) as u64)
}

#[cfg(windows)]
fn volume_available_bytes(data_dir: &Path) -> std::io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<_> = data_dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0;
    // SAFETY: path is a terminated UTF-16 string and available is writable.
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(available)
    }
}

#[cfg(not(any(unix, windows)))]
fn volume_available_bytes(_data_dir: &Path) -> std::io::Result<u64> {
    Ok(u64::MAX)
}

#[cfg(all(test, unix))]
#[path = "tests/disk_space_test.rs"]
pub(crate) mod tests;
