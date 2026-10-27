//! Metadata excluded from both shipped resources and installed ownership hashes.
pub(crate) fn ignored(name: &str) -> bool {
    name.starts_with('.') || name == "__pycache__" || name.ends_with(".pyc")
}
