use std::mem::size_of;
use std::ptr::null_mut;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

pub(crate) fn replace_file(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn create_remote_ssh_config_file(
    path: &std::path::Path,
) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

pub(crate) fn create_remote_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    use interprocess::os::windows::security_descriptor::{
        AsSecurityDescriptorExt as _, SecurityDescriptor,
    };
    use widestring::U16CString;

    let sddl = U16CString::from_str("D:P(A;OICI;GA;;;SY)(A;OICI;GA;;;OW)")
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let security_descriptor = SecurityDescriptor::deserialize(&sddl)?;
    let mut security_attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 0,
    };
    security_descriptor.write_to_security_attributes(&mut security_attributes);
    let path = extended_length_path(path)?;
    if unsafe { CreateDirectoryW(path.as_ptr(), &security_attributes) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(in crate::platform::windows) fn extended_length_path(
    path: &std::path::Path,
) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt as _;

    let path = std::path::absolute(path)?;
    let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let mut extended = if wide.starts_with(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16])
        || wide.starts_with(&[b'\\' as u16, b'\\' as u16, b'.' as u16, b'\\' as u16])
    {
        wide
    } else if wide.starts_with(&[b'\\' as u16, b'\\' as u16]) {
        "\\\\?\\UNC\\"
            .encode_utf16()
            .chain(wide.into_iter().skip(2))
            .collect()
    } else {
        "\\\\?\\".encode_utf16().chain(wide).collect()
    };
    extended.push(0);
    Ok(extended)
}
