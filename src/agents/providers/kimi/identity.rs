//! macOS st_dev is assigned at mount time; persist volume UUID + inode instead.
use super::{
    executable_record, hook_json, stamp, HomeRecord, HookContext, Stamp, EXECUTABLE_FILE, HOME_FILE,
};
use std::{ffi::CString, os::unix::ffi::OsStrExt, path::Path};

pub(super) fn volume_uuid(path: &Path) -> Result<[u8; 16], String> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: NUL-terminated path and a writable statfs output buffer.
    if unsafe { libc::statfs(path.as_ptr(), filesystem.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: statfs initialized the buffer on success, including f_mntonname.
    let filesystem = unsafe { filesystem.assume_init() };
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    // getattrlist returns a 4-byte length followed by the requested UUID.
    #[repr(C)]
    struct Volume {
        length: u32,
        uuid: [u8; 16],
    }
    let mut volume = Volume {
        length: 0,
        uuid: [0; 16],
    };
    // SAFETY: NUL-terminated path and valid, aligned buffers of the stated sizes.
    let result = unsafe {
        libc::getattrlist(
            filesystem.f_mntonname.as_ptr(),
            (&mut attributes as *mut libc::attrlist).cast(),
            (&mut volume as *mut Volume).cast(),
            std::mem::size_of::<Volume>(),
            0,
        )
    };
    if result != 0 {
        return Err(format!(
            "Kimi volume identity unavailable: {}",
            std::io::Error::last_os_error()
        ));
    }
    if volume.length != std::mem::size_of::<Volume>() as u32 || volume.uuid == [0; 16] {
        return Err("Kimi volume identity unavailable".into());
    }
    Ok(volume.uuid)
}

pub(super) fn home_matches(saved: &HomeRecord, current: &HomeRecord) -> bool {
    let mut comparable = current.clone();
    comparable.device = saved.device;
    if saved.volume_uuid.is_none() {
        comparable.volume_uuid = None;
    }
    comparable == *saved
}

pub(super) fn stamp_matches(saved: &Stamp, current: &Stamp) -> bool {
    let mut comparable = current.clone();
    comparable.device = saved.device;
    if saved.volume_uuid.is_none() {
        comparable.volume_uuid = None;
    }
    comparable == *saved
}

pub(super) fn verify_legacy_mount(
    hooks: &HookContext,
    saved: &HomeRecord,
    current: &HomeRecord,
) -> Result<(), String> {
    if saved.volume_uuid.is_none() && saved.device != current.device {
        // Old captures cannot attest a volume UUID retroactively. Permit only
        // the observed mount transition, corroborated by an unchanged executable
        // on the same old/current volume. All other directory/auth guards apply.
        let (path, executable) = executable_record(hooks)?;
        let now = stamp(&path)?;
        if executable.device != saved.device
            || now.device != current.device
            || !stamp_matches(&executable, &now)
        {
            return Err("Saved Kimi home changed; relaunch required".into());
        }
    }
    Ok(())
}

pub(super) fn upgrade_home(
    hooks: &HookContext,
    mut saved: HomeRecord,
    current: HomeRecord,
) -> Result<HomeRecord, String> {
    if saved.volume_uuid.is_none() {
        saved.volume_uuid = current.volume_uuid;
        let bytes = serde_json::to_vec(&saved).map_err(|e| e.to_string())?;
        hook_json::atomic_write(&hooks.spool.join(HOME_FILE), &bytes).map_err(|e| e.to_string())?;
    }
    Ok(saved)
}

pub(super) fn upgrade_executable(
    hooks: &HookContext,
    path: &Path,
    mut saved: Stamp,
    current: Stamp,
) -> Result<(), String> {
    if saved.volume_uuid.is_none() {
        // Keep the launch-time device number: a legacy home may still need it
        // to corroborate the old/current mapping, regardless of validation order.
        saved.volume_uuid = current.volume_uuid;
        let bytes = serde_json::to_vec(&(path, saved)).map_err(|e| e.to_string())?;
        hook_json::atomic_write(&hooks.spool.join(EXECUTABLE_FILE), &bytes)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
