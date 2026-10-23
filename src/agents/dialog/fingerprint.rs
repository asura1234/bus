//! Hashed-component diagnostics for an observation and its guarded write.
use super::Dialog;
use sha2::{Digest, Sha256};

pub(crate) fn trace(
    dialog: Option<&Dialog>,
    phase: &str,
    expected_digest: Option<&str>,
    content_revision: u64,
    pane: u32,
    synchronized_output: bool,
) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    // The native API can be called directly with a malformed expected digest.
    // Never let that arbitrary string become a payload dump in normal dev logs.
    let expected_digest_valid = expected_digest.is_none_or(valid_digest);
    let log_expected_digest = expected_digest.filter(|value| valid_digest(value));
    let Some(dialog) = dialog else {
        tracing::debug!(
            event = "bus.dialog.fingerprint",
            phase,
            pane,
            content_revision,
            synchronized_output,
            dialog_present = false,
            expected_digest = log_expected_digest,
            expected_digest_valid,
            "No native dialog at the fingerprint check"
        );
        return;
    };
    let digest = dialog.digest();
    // Keep selection separate from option content, so a cursor move is
    // distinguishable from a changed answer, checkbox, command or question.
    let options: Vec<_> = dialog
        .options
        .iter()
        .map(|option| {
            (
                option.number,
                &option.label,
                option.free_text,
                option.navigation_key,
                &option.identity_label,
            )
        })
        .collect();
    tracing::debug!(
        event = "bus.dialog.fingerprint", phase, pane, content_revision, synchronized_output,
        dialog_present = true, expected_digest = log_expected_digest, expected_digest_valid,
        digest_matches = expected_digest.map(|expected| expected == digest),
        dialog_digest = %digest, dialog_id = %dialog.id(), kind = ?dialog.kind,
        selected = ?dialog.selected(), option_count = dialog.options.len(),
        text_hash = %hash(&dialog.text), options_hash = %hash(&options),
        input_hash = %hash(&dialog.input), tabs_hash = %hash(&dialog.tabs),
        hint_hash = %hash(&dialog.hint), context_hash = %hash(&dialog.fingerprint_context),
        "Native dialog fingerprint components"
    );
}

fn hash(value: &impl std::fmt::Debug) -> String {
    format!("{:x}", Sha256::digest(format!("{value:?}").as_bytes()))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
