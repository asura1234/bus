//! Codex launch and observation-hook behavior, without room ownership.
pub(crate) mod hooks;
pub(crate) mod launch;
pub(crate) mod resume;
pub(crate) mod system_prompt;
pub(crate) mod usage;

#[cfg(test)]
#[path = "tests/hooks_test.rs"]
mod hooks_tests;
#[cfg(test)]
#[path = "tests/launch_test.rs"]
mod launch_tests;
