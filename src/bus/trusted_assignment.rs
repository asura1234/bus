//! Launch-time discovery env left over from the retired trusted room assignments.
//!
//! Nothing reads these values any more. `launch.rs` and `resume_launch.rs` still call
//! [`prepare_discovery`] and export [`AssignmentDiscovery::env`]; this module keeps that
//! surface byte-identical until those call sites are removed.
use std::path::{Path, PathBuf};

use super::{io, model::AgentId};

pub(crate) const ENV_BINARY: &str = "BUS_BINARY";
pub(crate) const ENV_DIRECTORY: &str = "BUS_TRUSTED_ASSIGNMENT_DIR";
pub(crate) const ENV_ENDPOINT: &str = "BUS_TRUSTED_ASSIGNMENT_ENDPOINT";
pub(crate) const ENV_TOKEN: &str = "BUS_TRUSTED_ASSIGNMENT_TOKEN";
const TOKEN_FILE: &str = ".read-token";

#[derive(Clone, Debug)]
pub(crate) struct AssignmentDiscovery {
    directory: PathBuf,
    endpoint: PathBuf,
    token: String,
}

impl AssignmentDiscovery {
    pub(crate) fn env(&self, binary: &Path) -> Vec<(String, String)> {
        vec![
            (ENV_BINARY.into(), binary.to_string_lossy().into_owned()),
            (
                ENV_DIRECTORY.into(),
                self.directory.to_string_lossy().into_owned(),
            ),
            (
                ENV_ENDPOINT.into(),
                self.endpoint.to_string_lossy().into_owned(),
            ),
            (ENV_TOKEN.into(), self.token.clone()),
        ]
    }
}

pub(crate) fn prepare_discovery(
    data_dir: &Path,
    agent_id: AgentId,
    launch_id: &str,
) -> Result<AssignmentDiscovery, String> {
    validate_identity(launch_id)?;
    let directory = data_dir
        .join("trusted-assignments")
        .join(agent_id.0.to_string())
        .join(launch_id);
    io::private_dir(&directory).map_err(|error| error.to_string())?;
    let token =
        io::digest(format!("{}:{}:{}", launch_id, std::process::id(), io::now_ns()).as_bytes());
    io::atomic_write(&directory.join(TOKEN_FILE), token.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(AssignmentDiscovery {
        endpoint: directory.join("active.json"),
        directory,
        token,
    })
}

fn validate_identity(value: &str) -> Result<(), String> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err("invalid launch identity".into())
    }
}
