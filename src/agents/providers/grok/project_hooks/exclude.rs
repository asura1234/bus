//! Grok supplies its registered global-source policy to the shared exclude writer.
use std::path::Path;
pub(super) const GROK_ENTRIES: [&str; 2] =
    ["/.grok/hooks/bus.json", "/.grok/hooks/.bus-hooks.lock"];
pub(super) fn ensure(root: &Path, home: &Path, entries: &[&str]) -> Result<(), String> {
    crate::agents::providers::git_exclude::ensure(root, entries, |directory, leaf| {
        super::roots::verify_exclude(directory, home, leaf)
    })
}
