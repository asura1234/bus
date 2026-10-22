//! Descriptor-relative I/O: no checked project component is followed by name.
use std::{
    ffi::{CString, OsStr},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path},
};

pub(super) struct Dir(File);

fn name(value: &OsStr) -> Result<CString, String> {
    CString::new(value.as_bytes()).map_err(|_| "Invalid hook path component".into())
}

fn opened(fd: libc::c_int) -> Result<File, String> {
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: a successful open/openat returned this new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

impl Dir {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        let mut dir = Self(File::open("/").map_err(|e| e.to_string())?);
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(component) => dir = dir.child(component, false)?,
                _ => return Err("Project hook root must be canonical and absolute".into()),
            }
        }
        Ok(dir)
    }

    fn open_at(&self, component: &OsStr, flags: libc::c_int) -> Result<File, String> {
        let component = name(component)?;
        // SAFETY: live directory descriptor, NUL-terminated single component,
        // and mode supplied for O_CREAT. Returned descriptor is uniquely owned.
        opened(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                component.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        })
    }

    pub(super) fn child(&self, component: impl AsRef<OsStr>, create: bool) -> Result<Self, String> {
        let component = component.as_ref();
        if create {
            let component = name(component)?;
            // SAFETY: live parent descriptor and a single NUL-terminated name.
            let result = unsafe { libc::mkdirat(self.0.as_raw_fd(), component.as_ptr(), 0o700) };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        self.open_at(component, libc::O_RDONLY | libc::O_DIRECTORY)
            .map(Self)
    }

    pub(super) fn same(&self, other: &Self) -> Result<bool, String> {
        same(&self.0, &other.0)
    }

    pub(super) fn lock(&self) -> Result<File, String> {
        let file = self.open_at(
            OsStr::new(".bus-hooks.lock"),
            libc::O_RDWR | libc::O_CREAT | libc::O_NONBLOCK,
        )?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Invalid project hook lock".into());
        }
        // SAFETY: descriptor remains owned by the returned lease. Nonblocking
        // refusal avoids freezing the server behind another setup process.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err("Project hooks are being changed; retry setup".into());
        }
        Ok(file)
    }

    pub(super) fn read(&self) -> Result<Option<(File, Vec<u8>)>, String> {
        let component = name(OsStr::new("bus.json"))?;
        // SAFETY: live directory descriptor and static single-component name.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                component.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        let mut file = opened(fd)?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Project hook is not a regular file".into());
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("Project hook file is too large".into());
        }
        Ok(Some((file, bytes)))
    }

    pub(super) fn replace(
        &self,
        bytes: &[u8],
        mut checkpoint: impl FnMut(super::Stage),
    ) -> Result<File, String> {
        static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = format!(".bus-hook-{}-{serial}.tmp", std::process::id());
        let mut file = self.open_at(
            OsStr::new(&temp),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )?;
        let source = name(OsStr::new(&temp))?;
        let target = name(OsStr::new("bus.json"))?;
        let result = (|| {
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            checkpoint(super::Stage::BeforeRename);
            // SAFETY: both names are relative to the pinned hooks directory.
            // renameat replaces the directory entry, never follows a leaf link.
            if unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    source.as_ptr(),
                    self.0.as_raw_fd(),
                    target.as_ptr(),
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            checkpoint(super::Stage::AfterRename);
            self.0.sync_all().map_err(|e| e.to_string())?;
            Ok(())
        })();
        // SAFETY: remove only our unique temporary name from the pinned folder;
        // after rename the name is absent and unlinkat harmlessly fails.
        unsafe {
            libc::unlinkat(self.0.as_raw_fd(), source.as_ptr(), 0);
        }
        result?;
        Ok(file)
    }
}

pub(super) fn same(left: &File, right: &File) -> Result<bool, String> {
    let left = left.metadata().map_err(|e| e.to_string())?;
    let right = right.metadata().map_err(|e| e.to_string())?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}
