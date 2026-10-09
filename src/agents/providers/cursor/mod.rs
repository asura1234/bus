//! Cursor launch, prompt delivery and hook observations, without room ownership.
pub(crate) mod final_reply;
pub(crate) mod hooks;
pub(crate) mod launch;
pub(crate) mod resume;
pub(crate) mod system_prompt;

#[cfg(test)]
#[path = "tests/hooks_test.rs"]
mod hooks_tests;
#[cfg(test)]
#[path = "tests/launch_test.rs"]
mod launch_tests;
#[cfg(test)]
#[path = "tests/system_prompt_test.rs"]
mod system_prompt_tests;
