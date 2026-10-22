//! Lane A+D supplies attested Grok capture reconstruction.
use crate::agents::providers::hook_json::HookContext;
use std::path::{Path, PathBuf};
pub(crate) fn capture_args(
    _hooks: &HookContext,
    _project: &Path,
) -> Result<(PathBuf, Vec<String>, bool), String> {
    Err("Grok resume capture is not implemented (G0; lane A+D)".into())
}
