//! Clock and content facts, independent of filesystem operations.
use sha2::{Digest, Sha256};

pub(crate) fn now_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

pub(crate) fn now_ms() -> u64 {
    (now_ns() / 1_000_000) as u64
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
