//! Codex's launch-time developer instruction points at the authored prompt file.
use std::path::Path;

pub(crate) fn prompt_args(path: &Path, adopted: bool) -> Result<Option<Vec<String>>, String> {
    // An adopted thread replays its original developer instructions; its caller
    // must deliver the authored prompt as a message instead.
    if adopted {
        return Ok(None);
    }
    // Keep the shell's typed command short; JSON escapes are valid TOML strings.
    let text = format!(
        "You are a Bus orchestrator. Your system prompt is the file {}. \
         Read all of it before you act on any message, and follow it for the \
         whole session.",
        path.display()
    );
    let value = serde_json::to_string(&text).map_err(|e| e.to_string())?;
    Ok(Some(vec![
        "-c".into(),
        format!("developer_instructions={value}"),
    ]))
}
