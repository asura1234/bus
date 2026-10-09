//! Claude prompt-file argv; prompt content and room policy belong to callers.

use std::path::Path;

/// Append to Claude's default prompt. An adopted session must render the
/// prompt fresh rather than replay its saved system-prompt snapshot.
pub(crate) fn prompt_args(path: &Path, adopted: bool) -> Vec<String> {
    let mut args = Vec::new();
    if adopted {
        args.extend(["--system-prompt-snapshot".into(), "off".into()]);
    }
    args.extend([
        "--append-system-prompt-file".into(),
        path.to_string_lossy().into_owned(),
    ]);
    args
}

#[cfg(test)]
#[path = "tests/system_prompt_test.rs"]
mod tests;
