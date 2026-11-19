//! System-prompt delivery contract for AGY workers.
use std::path::Path;

/// AGY has no CLI system-prompt flag (`--agent` only accepts discovered names
/// inside confined customization roots). Bus delivers worker onboarding as an
/// out-of-history first prompt via `deliver_launch_prompt`.
pub(crate) fn prompt_args(_path: &Path, _adopted: bool) -> Result<Option<Vec<String>>, String> {
    Ok(None)
}

pub(crate) fn prompt_message(text: &str) -> String {
    format!(
        "Bus: this is your system prompt; Bus could not set it at launch for this provider or session. Follow it for the rest of this session. Reply briefly once to confirm you are ready, then wait for room messages.\n\n{text}"
    )
}
