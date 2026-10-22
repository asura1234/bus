//! Lane E supplies the authored Grok Bus-wake prompt and bootstrap.
use std::path::Path;
pub(crate) fn orchestrator_prompt(_text: &str) -> Result<String, String> {
    Err("Grok orchestrator prompt is not implemented (G0; lane E)".into())
}
pub(crate) fn prompt_args(_path: &Path, _adopted: bool) -> Result<Option<Vec<String>>, String> {
    Err("Grok prompt installation is not implemented (G0; lane E)".into())
}
pub(crate) fn prompt_message(text: &str) -> String {
    format!("Bus: this is your system prompt; Bus could not set it at launch for this provider or session. Follow it for the rest of this session, then reply briefly that you are ready.\n\n{text}")
}
