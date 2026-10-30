//! Kimi accepts an agent file only for a new session. Resumed sessions retain it.
use std::path::Path;

pub(crate) fn prompt_args(path: &Path, adopted: bool) -> Result<Option<Vec<String>>, String> {
    if adopted {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let agent = path.with_file_name("kimi-agent.md");
    let body = format!("---\nname: bus-agent\ndescription: Bus room participant\n---\n{text}");
    crate::agents::providers::hook_json::atomic_write(&agent, body.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(Some(vec![
        "--agent-file".into(),
        agent.to_string_lossy().into_owned(),
    ]))
}
