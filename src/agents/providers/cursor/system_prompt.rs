//! Cursor receives the authored system prompt as its first message.
use std::path::Path;

pub(crate) fn prompt_args(_path: &Path, _adopted: bool) -> Result<Option<Vec<String>>, String> {
    Ok(None)
}

pub(crate) fn prompt_message(text: &str) -> String {
    format!(
        "Bus: this is your system prompt; Bus could not set it at launch for this provider or session. Follow it for the rest of this session, then reply briefly that you are ready.\n\n{text}"
    )
}
