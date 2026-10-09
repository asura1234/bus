pub(crate) use crate::platform::fs::AtomicWriteStage as SaveStage;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::messaging::model::BusState;

pub(crate) const STORE_VERSION: u64 = 1;

#[derive(Debug)]
pub(crate) enum StoreError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    SaveIo {
        path: PathBuf,
        stage: SaveStage,
        source: std::io::Error,
        bytes: usize,
    },
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
    UnsupportedVersion {
        path: PathBuf,
        found: u64,
    },
    ExistingStateUnreadable {
        path: PathBuf,
        detail: String,
    },
    UnsafePath(PathBuf),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "Bus storage {}: {source}", path.display())
            }
            Self::SaveIo {
                path,
                stage,
                source,
                ..
            } => {
                write!(
                    formatter,
                    "Bus storage {} ({}): {source}",
                    path.display(),
                    stage.name()
                )
            }
            Self::Corrupt { path, source } => {
                write!(formatter, "Corrupt Bus state {}: {source}", path.display())
            }
            Self::UnsupportedVersion { path, found } => write!(
                formatter,
                "Unsupported Bus state version {found} in {}",
                path.display()
            ),
            Self::ExistingStateUnreadable { path, detail } => write!(
                formatter,
                "Refusing to replace unreadable Bus state {}: {detail}",
                path.display()
            ),
            Self::UnsafePath(path) => {
                write!(formatter, "Unsafe Bus state path: {}", path.display())
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl StoreError {
    pub(crate) fn diagnostic(&self) -> (&'static str, Option<i32>, usize) {
        match self {
            Self::SaveIo {
                stage,
                source,
                bytes,
                ..
            } => (stage.name(), source.raw_os_error(), *bytes),
            Self::Io { source, .. } => ("read", source.raw_os_error(), 0),
            Self::Corrupt { .. } => ("parse", None, 0),
            Self::UnsupportedVersion { .. } => ("version", None, 0),
            Self::ExistingStateUnreadable { .. } => ("preflight", None, 0),
            Self::UnsafePath(_) => ("path", None, 0),
        }
    }

    pub(crate) fn retryable_io(&self) -> bool {
        matches!(self, Self::Io { .. } | Self::SaveIo { .. })
    }
}

#[derive(Deserialize, Serialize)]
struct StoredDocument {
    version: u64,
    state: BusState,
}

pub(crate) struct JsonStore {
    path: PathBuf,
    #[cfg(test)]
    fail_stage: std::sync::Mutex<Option<SaveStage>>,
}

impl JsonStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            #[cfg(test)]
            fail_stage: std::sync::Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn fail_once_at(&self, stage: SaveStage) {
        *self.fail_stage.lock().unwrap() = Some(stage);
    }

    fn injected_failure(&self, stage: SaveStage) -> std::io::Result<()> {
        #[cfg(test)]
        {
            let mut fail_stage = self.fail_stage.lock().unwrap();
            if *fail_stage == Some(stage) {
                *fail_stage = None;
                return Err(std::io::Error::from_raw_os_error(28));
            }
        }
        let _ = stage;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn load(&self) -> Result<Option<BusState>, StoreError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(StoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let value = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|source| {
            StoreError::Corrupt {
                path: self.path.clone(),
                source,
            }
        })?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| StoreError::Corrupt {
                path: self.path.clone(),
                source: serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "missing numeric Bus state version",
                )),
            })?;
        if version != STORE_VERSION {
            return Err(StoreError::UnsupportedVersion {
                path: self.path.clone(),
                found: version,
            });
        }
        let document = serde_json::from_value::<StoredDocument>(value).map_err(|source| {
            StoreError::Corrupt {
                path: self.path.clone(),
                source,
            }
        })?;
        Ok(Some(document.state))
    }

    pub(crate) fn save(&self, state: &BusState) -> Result<(), StoreError> {
        if self.path.exists() {
            self.load()
                .map_err(|error| StoreError::ExistingStateUnreadable {
                    path: self.path.clone(),
                    detail: error.to_string(),
                })?;
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(StoreError::UnsafePath(self.path.clone()));
            }
        }
        let parent = self.path.parent().ok_or_else(|| StoreError::SaveIo {
            path: self.path.clone(),
            stage: SaveStage::Parent,
            bytes: 0,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Bus state path has no parent directory",
            ),
        })?;
        std::fs::create_dir_all(parent).map_err(|source| StoreError::SaveIo {
            path: parent.to_path_buf(),
            stage: SaveStage::Parent,
            bytes: 0,
            source,
        })?;
        let bytes = serde_json::to_vec_pretty(&StoredDocument {
            version: STORE_VERSION,
            state: state.clone(),
        })
        .map_err(|source| StoreError::Corrupt {
            path: self.path.clone(),
            source,
        })?;
        crate::platform::fs::atomic_write_detailed_with_checkpoint(&self.path, &bytes, |stage| {
            self.injected_failure(stage)
        })
        .map_err(|error| StoreError::SaveIo {
            path: error.path,
            stage: error.stage,
            bytes: bytes.len(),
            source: error.source,
        })
    }
}

#[cfg(test)]
#[path = "tests/state_store_test.rs"]
mod tests;
