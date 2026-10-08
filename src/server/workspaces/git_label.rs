#[path = "git_discovery.rs"]
mod discovery;
#[cfg(test)]
#[path = "tests/git_support.rs"]
pub(super) mod test_support;

pub(crate) use self::discovery::automatic_workspace_label;

pub use self::discovery::{fallback_label_from_cwd, git_space_metadata};
