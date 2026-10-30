//! Grok Build integration; native completion acceptance awaits signed-in evidence.
pub(crate) mod completion;
pub(crate) mod hooks;
pub(crate) mod launch;
mod project_hooks;
pub(crate) mod resume;
#[cfg(feature = "dev-tools")]
pub(crate) mod synthetic;
pub(crate) mod system_prompt;
pub(crate) mod usage;
use super::hook_json::{HookContract, HookEntryShape};
pub(crate) const HOOK_CONTRACT: HookContract = HookContract {
    adapter: "grok-hook",
    events: &[
        "SessionStart",
        "UserPromptSubmit",
        "Stop",
        "StopFailure",
        "StopCancelled",
        "Notification",
    ],
    entry_shape: HookEntryShape::NestedCommands,
    default_version: None,
};
