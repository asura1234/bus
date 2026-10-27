//! Metadata excluded from both shipped resources and installed ownership hashes.
pub(crate) fn ignored(name: &str) -> bool {
    matches!(name, ".DS_Store" | "Thumbs.db" | "__pycache__")
}
