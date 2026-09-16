//! Opt-in Orchestrator control capability: the raw token never outlives launch.

use std::{
    fmt,
    sync::{Mutex, OnceLock},
};

pub(crate) const TOKEN_ENV_VAR: &str = "BUS_ORCHESTRATOR_CONTROL_TOKEN";
const MIN_TOKEN_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 4096;
/// A launch secret must not be a repeated or alternating pattern; hex and base64
/// tokens of the required length carry far more distinct bytes than this floor.
const MIN_DISTINCT_BYTES: usize = 12;

/// Only the SHA-256 digest of the launch token is retained; the raw secret is dropped.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct TokenDigest(String);

impl TokenDigest {
    /// Digest comparison is the only production use; the hex form exists for tests.
    #[cfg(test)]
    pub(crate) fn hex(&self) -> &str {
        &self.0
    }

    /// Compares digests rather than secrets, in time independent of the match position.
    pub(crate) fn matches(&self, presented: &str) -> bool {
        let presented = super::io::digest(presented.as_bytes());
        let expected = self.0.as_bytes();
        let presented = presented.as_bytes();
        if expected.len() != presented.len() {
            return false;
        }
        let mut difference = 0u8;
        for (left, right) in expected.iter().zip(presented) {
            difference |= left ^ right;
        }
        difference == 0
    }
}

impl fmt::Debug for TokenDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenDigest")
            .field("digest", &self.0)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

fn armed_slot() -> &'static Mutex<Option<TokenDigest>> {
    static ARMED: OnceLock<Mutex<Option<TokenDigest>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(None))
}

pub(crate) fn validate_and_hash(token: &str) -> Result<TokenDigest, String> {
    if token.trim() != token {
        return Err(format!(
            "{TOKEN_ENV_VAR} must not be padded with whitespace"
        ));
    }
    if token.len() < MIN_TOKEN_BYTES || token.len() > MAX_TOKEN_BYTES {
        return Err(format!(
            "{TOKEN_ENV_VAR} must contain {MIN_TOKEN_BYTES}–{MAX_TOKEN_BYTES} bytes"
        ));
    }
    if token.bytes().any(|byte| byte.is_ascii_whitespace()) || token.chars().any(char::is_control) {
        return Err(format!(
            "{TOKEN_ENV_VAR} must not contain whitespace or control characters"
        ));
    }
    let mut seen = [false; 256];
    for byte in token.bytes() {
        seen[usize::from(byte)] = true;
    }
    if seen.iter().filter(|present| **present).count() < MIN_DISTINCT_BYTES {
        return Err(format!(
            "{TOKEN_ENV_VAR} must be a high-entropy secret, not a repeated pattern"
        ));
    }
    Ok(TokenDigest(super::io::digest(token.as_bytes())))
}

/// Reads the launch token once, retains only its digest, and scrubs the raw secret
/// from this process so no child launch can inherit it.
pub(crate) fn arm_from_environment() -> Result<TokenDigest, String> {
    // A protected launch never inherits an earlier digest, and a rejected token
    // leaves nothing armed behind it.
    clear_armed();
    let token = std::env::var(TOKEN_ENV_VAR)
        .map_err(|_| format!("{TOKEN_ENV_VAR} is required by --orchestrator-control"))?;
    std::env::remove_var(TOKEN_ENV_VAR);
    let digest = validate_and_hash(&token)?;
    *armed_slot().lock().map_err(|error| error.to_string())? = Some(digest.clone());
    Ok(digest)
}

/// Read by the external control client from its own environment. The protected
/// instance keeps only a digest, so the raw secret lives solely in the caller.
pub(crate) fn capability_from_environment() -> Option<String> {
    std::env::var(TOKEN_ENV_VAR)
        .ok()
        .filter(|token| !token.is_empty())
}

/// Adoption is one-shot: the coordinator that starts first consumes the digest, so a
/// later worker in the same process cannot inherit the boundary.
pub(crate) fn take_armed() -> Option<TokenDigest> {
    armed_slot().lock().ok().and_then(|mut slot| slot.take())
}

fn clear_armed() {
    if let Ok(mut slot) = armed_slot().lock() {
        *slot = None;
    }
}

#[cfg(test)]
pub(crate) fn disarm_for_test() {
    clear_armed();
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "Rk9vQmFyOTdaeDNRd0x1TnBFc1R2MmhKZGtDeQ";

    #[test]
    fn room_orchestrator_control_token_requires_thirty_two_high_entropy_bytes() {
        assert!(validate_and_hash(GOOD).is_ok());
        for weak in [
            "",
            "   ",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abababababababababababababababababab",
            "Rk9vQmFyOTdaeDNRd0x1TnBFc1R2MmhK ZGtDeQ",
            "Rk9vQmFyOTdaeDNRd0x1TnBFc1R2MmhKZGtD\ney",
        ] {
            assert!(
                validate_and_hash(weak).is_err(),
                "weak token accepted: {weak:?}"
            );
        }
        let short = &GOOD[..31];
        assert!(validate_and_hash(short).is_err(), "31 bytes accepted");
    }

    #[test]
    fn room_orchestrator_control_token_hashes_and_never_exposes_raw_secret() {
        let digest = validate_and_hash(GOOD).unwrap();
        assert_eq!(digest.hex(), crate::bus::io::digest(GOOD.as_bytes()));
        assert!(digest.matches(GOOD));
        assert!(!digest.matches("Rk9vQmFyOTdaeDNRd0x1TnBFc1R2MmhKZGtDeX"));
        assert!(!digest.matches(""));
        let rendered = format!("{digest:?}");
        assert!(!rendered.contains(GOOD), "{rendered}");
    }

    #[test]
    fn room_orchestrator_control_arm_removes_raw_token_from_process_environment() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        std::env::set_var(TOKEN_ENV_VAR, GOOD);
        let digest = arm_from_environment().unwrap();
        assert_eq!(digest.hex(), crate::bus::io::digest(GOOD.as_bytes()));
        assert!(
            std::env::var_os(TOKEN_ENV_VAR).is_none(),
            "raw token survived launch"
        );
        assert_eq!(take_armed().unwrap().hex(), digest.hex());
        disarm_for_test();
    }

    #[test]
    fn room_orchestrator_control_armed_capability_is_taken_exactly_once() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        std::env::set_var(TOKEN_ENV_VAR, GOOD);

        let digest = arm_from_environment().unwrap();

        assert_eq!(take_armed().unwrap().hex(), digest.hex());
        assert!(
            take_armed().is_none(),
            "a second adopter inherited the launch capability"
        );
        std::env::remove_var(TOKEN_ENV_VAR);
    }

    #[test]
    fn room_orchestrator_control_failed_arm_clears_stale_armed_state() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        std::env::set_var(TOKEN_ENV_VAR, GOOD);
        arm_from_environment().unwrap();

        // A later protected launch without a token must not inherit the prior digest.
        std::env::remove_var(TOKEN_ENV_VAR);
        assert!(arm_from_environment().is_err());
        assert!(
            take_armed().is_none(),
            "stale capability survived a missing token"
        );

        std::env::set_var(TOKEN_ENV_VAR, GOOD);
        arm_from_environment().unwrap();
        std::env::set_var(TOKEN_ENV_VAR, "weak");
        assert!(arm_from_environment().is_err());
        assert!(
            take_armed().is_none(),
            "stale capability survived a rejected token"
        );
        std::env::remove_var(TOKEN_ENV_VAR);
    }

    #[test]
    fn room_orchestrator_control_arm_fails_closed_without_a_token() {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        std::env::remove_var(TOKEN_ENV_VAR);
        let error = arm_from_environment().unwrap_err();
        assert!(error.contains(TOKEN_ENV_VAR), "{error}");
        assert!(take_armed().is_none());
    }
}
