//! Grok provider lane contracts; G0 is an explicitly incomplete scaffold.
pub(crate) mod completion;
pub(crate) mod hooks;
pub(crate) mod launch;
pub(crate) mod resume;
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
