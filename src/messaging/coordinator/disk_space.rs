//! One background free-space query per minute; the coordinator never waits for it.
use super::{mpsc, BusEvent, Worker};
use crate::messaging::prefs::settings;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

const SAMPLE_EVERY: Duration = Duration::from_secs(60);

type Source = Arc<dyn Fn(&Path) -> std::io::Result<u64> + Send + Sync>;

pub(super) struct DiskSpace {
    source: Source,
    pending: Option<mpsc::Receiver<std::io::Result<u64>>>,
    last_started: Option<Instant>,
}

impl Default for DiskSpace {
    fn default() -> Self {
        // Unit tests must opt into a synthetic source, never query the host disk.
        #[cfg(test)]
        let source: Source = Arc::new(|_| Ok(u64::MAX));
        #[cfg(not(test))]
        let source: Source = Arc::new(available_bytes);
        Self::with_source(source)
    }
}

impl DiskSpace {
    pub(super) fn with_source(source: Source) -> Self {
        Self {
            source,
            pending: None,
            last_started: None,
        }
    }

    #[cfg(test)]
    pub(super) fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    fn poll(&mut self, data_dir: &Path, now: Instant) -> Option<std::io::Result<u64>> {
        if let Some(receiver) = &self.pending {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(std::io::Error::other("Disk sampler disconnected"))
                }
            };
            self.pending = None;
            return Some(result);
        }
        if self
            .last_started
            .is_some_and(|last| now.saturating_duration_since(last) < SAMPLE_EVERY)
        {
            return None;
        }
        self.last_started = Some(now);
        let source = self.source.clone();
        let dir = data_dir.to_path_buf();
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("bus-disk-space".into())
            .spawn(move || {
                let _ = sender.send(source(&dir));
            }) {
            Ok(_) => {
                self.pending = Some(receiver);
                None
            }
            Err(error) => Some(Err(error)),
        }
    }
}

impl Worker {
    pub(super) fn refresh_disk_space_at(&mut self, now: Instant) -> Result<(), String> {
        let Some(result) = self.disk_space.poll(&self.data_dir, now) else {
            return Ok(());
        };
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(event = "bus.disk_space.failed", %error, "Disk measurement failed; retaining alert state");
                return Ok(());
            }
        };
        let saved = self
            .settings_path
            .as_deref()
            .and_then(|path| settings::load(path).ok())
            .unwrap_or_default();
        let mut state = self.state.clone();
        if state
            .notify_disk_space(
                bytes,
                saved.disk_warning_gb,
                saved.disk_critical_gb,
                super::io::now_ms(),
            )
            .map_err(|e| e.to_string())?
        {
            self.save(state)?;
        }
        Ok(())
    }

    pub(super) fn set_disk_threshold(
        &mut self,
        warning: bool,
        value: u32,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let path = self
            .settings_path
            .as_deref()
            .ok_or("Bus settings location unavailable")?;
        let saved = settings::update(path, |saved| {
            if warning {
                saved.disk_warning_gb = value;
            } else {
                saved.disk_critical_gb = value;
            }
        })?;
        let _ = events.send(BusEvent::SettingsChanged(saved));
        Ok(())
    }
}

#[cfg(not(test))]
fn available_bytes(data_dir: &Path) -> std::io::Result<u64> {
    // The TUI driver owns this marker and an isolated data dir. Its dev build
    // measures only this fixture, so scratch acceptance never reads real capacity.
    #[cfg(feature = "dev-tools")]
    if std::env::var_os("BUS_TUI_OWNER").is_some() {
        return match std::fs::read_to_string(data_dir.join("disk-free-bytes")) {
            Ok(value) => value
                .trim()
                .parse()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(u64::MAX),
            Err(error) => Err(error),
        };
    }
    volume_available_bytes(data_dir)
}

#[cfg(all(unix, not(test)))]
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

#[cfg(all(windows, not(test)))]
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
