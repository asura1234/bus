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
    crate::utils::disk_space::available_bytes(data_dir)
}
