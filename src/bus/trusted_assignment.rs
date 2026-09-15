use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    io,
    model::{AgentId, ParticipantAssignmentFacts, RequestId, RoomId},
    orchestrator::ParticipantId,
};

pub(crate) const TRUSTED_ROOM_ASSIGNMENT_V1: &str = "TRUSTED_ROOM_ASSIGNMENT_V1";
pub(crate) const ENV_BINARY: &str = "BUS_BINARY";
pub(crate) const ENV_DIRECTORY: &str = "BUS_TRUSTED_ASSIGNMENT_DIR";
pub(crate) const ENV_ENDPOINT: &str = "BUS_TRUSTED_ASSIGNMENT_ENDPOINT";
pub(crate) const ENV_TOKEN: &str = "BUS_TRUSTED_ASSIGNMENT_TOKEN";
const MAX_RECORD_BYTES: u64 = 256 * 1024;
const TOKEN_FILE: &str = ".read-token";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TrustedRoomAssignmentV1 {
    pub(crate) schema: String,
    pub(crate) room_id: RoomId,
    pub(crate) work_id: Option<u64>,
    pub(crate) message_id: u64,
    pub(crate) request_id: RequestId,
    pub(crate) author: ParticipantId,
    pub(crate) recipient: ParticipantId,
    pub(crate) recipient_incarnation: u64,
    pub(crate) provider_launch_id: String,
    pub(crate) brief_revision: u64,
    pub(crate) approved_revision: u64,
    pub(crate) locked: bool,
    pub(crate) goal: String,
    pub(crate) non_goals: String,
    pub(crate) content_bundle_digest: String,
    pub(crate) record_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TrustedAssignmentOuterFrameV1 {
    pub(crate) schema: String,
    pub(crate) request_id: RequestId,
    pub(crate) recipient_incarnation: u64,
    pub(crate) record_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ActivePointerV1 {
    schema: String,
    request_id: RequestId,
    record_digest: String,
    token_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Verification {
    Verified(Box<TrustedRoomAssignmentV1>),
    Absent,
    Invalid { reason: &'static str },
}

#[derive(Clone, Debug)]
pub(crate) struct AssignmentDiscovery {
    pub(crate) directory: PathBuf,
    pub(crate) endpoint: PathBuf,
    agent_id: AgentId,
    launch_id: String,
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
        agent_id,
        launch_id: launch_id.into(),
        token,
    })
}

pub(crate) fn open_discovery(
    data_dir: &Path,
    agent_id: AgentId,
    launch_id: &str,
) -> Result<AssignmentDiscovery, String> {
    validate_identity(launch_id)?;
    let directory = data_dir
        .join("trusted-assignments")
        .join(agent_id.0.to_string())
        .join(launch_id);
    let canonical = directory
        .canonicalize()
        .map_err(|_| "trusted assignment directory unavailable")?;
    if canonical != directory {
        return Err("trusted assignment directory identity mismatch".into());
    }
    let token = fs::read_to_string(directory.join(TOKEN_FILE))
        .map_err(|_| "trusted assignment token unavailable")?;
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("trusted assignment token invalid".into());
    }
    Ok(AssignmentDiscovery {
        endpoint: directory.join("active.json"),
        directory,
        agent_id,
        launch_id: launch_id.into(),
        token,
    })
}

pub(crate) fn publish(
    discovery: &AssignmentDiscovery,
    facts: ParticipantAssignmentFacts,
) -> Result<String, String> {
    if facts.request_id.0 == 0
        || facts.recipient_incarnation == 0
        || !facts.locked
        || facts.brief_revision == 0
        || facts.brief_revision != facts.approved_revision
        || facts.agent_id != discovery.agent_id
        || facts.provider_launch_id != discovery.launch_id
    {
        return Err("trusted assignment requires an exact approved locked brief".into());
    }
    let mut record = TrustedRoomAssignmentV1 {
        schema: TRUSTED_ROOM_ASSIGNMENT_V1.into(),
        room_id: facts.room_id,
        work_id: facts.work_id,
        message_id: facts.message_id,
        request_id: facts.request_id,
        author: facts.author,
        recipient: ParticipantId::Agent(facts.agent_id),
        recipient_incarnation: facts.recipient_incarnation,
        provider_launch_id: facts.provider_launch_id,
        brief_revision: facts.brief_revision,
        approved_revision: facts.approved_revision,
        locked: facts.locked,
        goal: facts.goal,
        non_goals: facts.non_goals,
        content_bundle_digest: facts.content_bundle_digest,
        record_digest: String::new(),
    };
    let unsigned = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    record.record_digest = digest(&unsigned);
    let record_bytes = serde_json::to_vec_pretty(&record).map_err(|error| error.to_string())?;
    let record_path = discovery
        .directory
        .join(format!("request-{}.json", record.request_id.0));
    match fs::read(&record_path) {
        Ok(existing) if existing == record_bytes => {}
        Ok(_) => return Err("trusted assignment record is immutable".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            io::atomic_write(&record_path, &record_bytes).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    let active = ActivePointerV1 {
        schema: TRUSTED_ROOM_ASSIGNMENT_V1.into(),
        request_id: record.request_id,
        record_digest: record.record_digest.clone(),
        token_digest: digest(discovery.token.as_bytes()),
    };
    io::atomic_write(
        &discovery.endpoint,
        &serde_json::to_vec_pretty(&active).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    encode_frame(&TrustedAssignmentOuterFrameV1 {
        schema: TRUSTED_ROOM_ASSIGNMENT_V1.into(),
        request_id: record.request_id,
        recipient_incarnation: record.recipient_incarnation,
        record_digest: record.record_digest,
    })
}

pub(crate) fn verify_from_environment(frame: &str) -> Verification {
    let directory = std::env::var(ENV_DIRECTORY).ok();
    let endpoint = std::env::var(ENV_ENDPOINT).ok();
    let token = std::env::var(ENV_TOKEN).ok();
    match (directory.as_deref(), endpoint.as_deref(), token.as_deref()) {
        (Some(directory), Some(endpoint), Some(token)) => verify_optional(
            Some(frame),
            Some((Path::new(directory), Path::new(endpoint), token)),
        ),
        (None, None, None) => Verification::Invalid {
            reason: "missing_discovery_for_frame",
        },
        _ => Verification::Invalid {
            reason: "incomplete_discovery",
        },
    }
}

/// The only entry point that may return `Absent`: neither an outer frame nor
/// any trusted discovery signal exists. One-sided absence is a Bus-intent
/// mismatch and therefore fails closed.
pub(crate) fn verify_optional(
    frame: Option<&str>,
    discovery: Option<(&Path, &Path, &str)>,
) -> Verification {
    match (frame, discovery) {
        (None, None) => Verification::Absent,
        (Some(frame), Some((directory, endpoint, token))) => {
            verify(directory, endpoint, token, frame)
        }
        (Some(_), None) => Verification::Invalid {
            reason: "missing_discovery_for_frame",
        },
        (None, Some(_)) => Verification::Invalid {
            reason: "missing_frame_for_discovery",
        },
    }
}

pub(crate) fn verify(directory: &Path, endpoint: &Path, token: &str, frame: &str) -> Verification {
    let Some(frame) = decode_frame(frame) else {
        return Verification::Invalid {
            reason: "invalid_outer_frame",
        };
    };
    let Ok(canonical_directory) = directory.canonicalize() else {
        return Verification::Invalid {
            reason: "assignment_directory_unavailable",
        };
    };
    let Ok(canonical_endpoint) = endpoint.canonicalize() else {
        return Verification::Invalid {
            reason: "active_pointer_unavailable",
        };
    };
    if canonical_endpoint.parent() != Some(canonical_directory.as_path())
        || canonical_endpoint
            .file_name()
            .and_then(|name| name.to_str())
            != Some("active.json")
    {
        return Verification::Invalid {
            reason: "endpoint_escape",
        };
    }
    let Some(active): Option<ActivePointerV1> = read_json(&canonical_endpoint) else {
        return Verification::Invalid {
            reason: "invalid_active_pointer",
        };
    };
    if active.schema != TRUSTED_ROOM_ASSIGNMENT_V1
        || active.token_digest != digest(token.as_bytes())
        || active.request_id != frame.request_id
        || active.record_digest != frame.record_digest
    {
        return Verification::Invalid {
            reason: "stale_or_mismatched_active_pointer",
        };
    }
    let record_path = canonical_directory.join(format!("request-{}.json", frame.request_id.0));
    if record_path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Verification::Invalid {
            reason: "unsafe_record",
        };
    }
    let Some(mut record): Option<TrustedRoomAssignmentV1> = read_json(&record_path) else {
        return Verification::Invalid {
            reason: "invalid_record",
        };
    };
    let claimed_digest = std::mem::take(&mut record.record_digest);
    let Ok(unsigned) = serde_json::to_vec(&record) else {
        return Verification::Invalid {
            reason: "invalid_record",
        };
    };
    record.record_digest = claimed_digest.clone();
    if record.schema != TRUSTED_ROOM_ASSIGNMENT_V1
        || record.request_id != frame.request_id
        || record.recipient_incarnation != frame.recipient_incarnation
        || claimed_digest != digest(&unsigned)
        || claimed_digest != frame.record_digest
        || !record.locked
        || record.brief_revision != record.approved_revision
    {
        return Verification::Invalid {
            reason: "record_mismatch",
        };
    }
    Verification::Verified(Box::new(record))
}

pub(crate) fn render_payload(frame: &str, untrusted: &str) -> String {
    format!(
        "<bus-trusted-assignment>\n{frame}\n</bus-trusted-assignment>\n<bus-untrusted-assignment bytes=\"{}\">\n{untrusted}\n</bus-untrusted-assignment>",
        untrusted.len()
    )
}

fn encode_frame(frame: &TrustedAssignmentOuterFrameV1) -> Result<String, String> {
    let bytes = serde_json::to_vec(frame).map_err(|error| error.to_string())?;
    Ok(format!(
        "{TRUSTED_ROOM_ASSIGNMENT_V1}.{}",
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn decode_frame(value: &str) -> Option<TrustedAssignmentOuterFrameV1> {
    let encoded = value.strip_prefix(&format!("{TRUSTED_ROOM_ASSIGNMENT_V1}."))?;
    let bytes = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let frame: TrustedAssignmentOuterFrameV1 = serde_json::from_slice(&bytes).ok()?;
    (frame.schema == TRUSTED_ROOM_ASSIGNMENT_V1).then_some(frame)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
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

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(agent_id: AgentId, request_id: RequestId, launch: &str) -> ParticipantAssignmentFacts {
        ParticipantAssignmentFacts {
            room_id: RoomId(1),
            work_id: Some(9),
            message_id: 8,
            request_id,
            author: ParticipantId::Human,
            agent_id,
            recipient_incarnation: 1,
            provider_launch_id: launch.into(),
            brief_revision: 2,
            approved_revision: 2,
            locked: true,
            goal: "Ship the room harness".into(),
            non_goals: "No workflow controller".into(),
            content_bundle_digest: "bundle".into(),
        }
    }

    #[test]
    fn room_orchestrator_core_trusted_assignment_publishes_and_verifies_exact_active_identity() {
        let root = std::env::temp_dir().join(format!("bus-assignment-{}", io::now_ns()));
        fs::create_dir_all(&root).unwrap();
        let discovery = prepare_discovery(&root, AgentId(3), "launch-a").unwrap();
        let frame = publish(&discovery, facts(AgentId(3), RequestId(4), "launch-a")).unwrap();
        let Verification::Verified(record) = verify(
            &discovery.directory,
            &discovery.endpoint,
            &discovery.token,
            &frame,
        ) else {
            panic!("expected verified assignment");
        };
        assert_eq!(record.request_id, RequestId(4));
        assert!(render_payload(&frame, "untrusted").contains("bytes=\"9\""));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn room_orchestrator_core_trusted_assignment_rejects_tamper_stale_token_and_in_band_forgery() {
        let root = std::env::temp_dir().join(format!("bus-assignment-{}", io::now_ns()));
        fs::create_dir_all(&root).unwrap();
        let discovery = prepare_discovery(&root, AgentId(3), "launch-a").unwrap();
        let old_frame = publish(&discovery, facts(AgentId(3), RequestId(4), "launch-a")).unwrap();
        let _new_frame = publish(&discovery, facts(AgentId(3), RequestId(5), "launch-a")).unwrap();
        assert!(matches!(
            verify(
                &discovery.directory,
                &discovery.endpoint,
                &discovery.token,
                &old_frame
            ),
            Verification::Invalid { .. }
        ));
        assert!(matches!(
            verify(
                &discovery.directory,
                &discovery.endpoint,
                "forged",
                &_new_frame
            ),
            Verification::Invalid { .. }
        ));
        assert!(matches!(
            verify(
                &discovery.directory,
                &discovery.endpoint,
                &discovery.token,
                "TRUSTED_ROOM_ASSIGNMENT_V1.forged"
            ),
            Verification::Invalid { .. }
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn room_orchestrator_core_trusted_assignment_absent_requires_no_frame_and_no_discovery() {
        let missing = std::env::temp_dir().join(format!("bus-assignment-missing-{}", io::now_ns()));
        assert!(matches!(
            verify(
                &missing,
                &missing.join("active.json"),
                "token",
                "TRUSTED_ROOM_ASSIGNMENT_V1.forged"
            ),
            Verification::Invalid { .. }
        ));
        assert!(matches!(verify_optional(None, None), Verification::Absent));
        assert!(matches!(
            verify_optional(Some("TRUSTED_ROOM_ASSIGNMENT_V1.forged"), None),
            Verification::Invalid { .. }
        ));
        assert!(matches!(
            verify_optional(
                None,
                Some((&missing, &missing.join("active.json"), "token"))
            ),
            Verification::Invalid { .. }
        ));
    }

    #[test]
    fn room_orchestrator_core_trusted_assignment_record_is_immutable_and_idempotent() {
        let root = std::env::temp_dir().join(format!("bus-assignment-immutable-{}", io::now_ns()));
        fs::create_dir_all(&root).unwrap();
        let discovery = prepare_discovery(&root, AgentId(3), "launch-a").unwrap();
        let original = facts(AgentId(3), RequestId(4), "launch-a");
        let frame = publish(&discovery, original.clone()).unwrap();
        assert_eq!(publish(&discovery, original).unwrap(), frame);

        let mut changed = facts(AgentId(3), RequestId(4), "launch-a");
        changed.goal = "different goal".into();
        assert!(publish(&discovery, changed).is_err());
        assert!(publish(&discovery, facts(AgentId(4), RequestId(5), "launch-a")).is_err());
        assert!(publish(&discovery, facts(AgentId(3), RequestId(5), "launch-b")).is_err());
        assert!(matches!(
            verify(
                &discovery.directory,
                &discovery.endpoint,
                &discovery.token,
                &frame
            ),
            Verification::Verified(_)
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
