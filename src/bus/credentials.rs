use std::{
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::io;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredCredential {
    generation: u64,
    api_key: String,
}

#[derive(Clone)]
pub(crate) struct CredentialGeneration {
    secret: String,
    pub(crate) generation: u64,
    pub(crate) digest: String,
}

impl CredentialGeneration {
    pub(crate) fn expose_for_provider(&self) -> &str {
        &self.secret
    }
}

impl fmt::Debug for CredentialGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialGeneration")
            .field("generation", &self.generation)
            .field("digest", &self.digest)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir
                .join("private")
                .join("orchestrator-credentials.json"),
        }
    }

    pub(crate) fn replace(&self, api_key: &str) -> Result<CredentialGeneration, String> {
        validate_key(api_key)?;
        let generation = self
            .load()
            .map_or(1, |credential| credential.generation.saturating_add(1));
        let parent = self.path.parent().ok_or("credential parent unavailable")?;
        io::private_dir(parent).map_err(|error| error.to_string())?;
        let stored = StoredCredential {
            generation,
            api_key: api_key.into(),
        };
        io::atomic_write(
            &self.path,
            &serde_json::to_vec(&stored).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        restrict_file(&self.path)?;
        self.load()
            .ok_or_else(|| "credential persistence failed".into())
    }

    pub(crate) fn load(&self) -> Option<CredentialGeneration> {
        let bytes = std::fs::read(&self.path).ok()?;
        if bytes.len() > 16 * 1024 {
            return None;
        }
        let stored: StoredCredential = serde_json::from_slice(&bytes).ok()?;
        validate_key(&stored.api_key).ok()?;
        Some(CredentialGeneration {
            digest: format!("{:x}", Sha256::digest(stored.api_key.as_bytes())),
            secret: stored.api_key,
            generation: stored.generation,
        })
    }
}

fn validate_key(api_key: &str) -> Result<(), String> {
    if api_key.trim() != api_key
        || api_key.len() < 8
        || api_key.len() > 4096
        || api_key.contains(['\r', '\n', '\0'])
    {
        Err("DeepSeek API key is invalid".into())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_orchestrator_core_private_credential_reloads_by_generation_and_redacts() {
        let root = std::env::temp_dir().join(format!("bus-credential-{}", io::now_ns()));
        std::fs::create_dir_all(&root).unwrap();
        let store = CredentialStore::new(&root);
        let first = store.replace("test-secret-one").unwrap();
        let second = store.replace("test-secret-two").unwrap();
        assert_eq!(first.generation, 1);
        assert_eq!(second.generation, 2);
        assert_eq!(
            store.load().unwrap().expose_for_provider(),
            "test-secret-two"
        );
        assert!(!format!("{second:?}").contains("test-secret-two"));
        let _ = std::fs::remove_dir_all(root);
    }
}
