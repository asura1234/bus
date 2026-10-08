pub(crate) mod attachments;
pub(crate) mod control;
pub(crate) mod coordinator;
pub(crate) mod diagnostics;
pub(crate) mod model;
pub(crate) mod native;
pub(crate) mod orchestration;
pub(crate) mod prefs;
pub(crate) mod provider_glue;
pub(crate) mod storage;

// Temporary names for consumers moved in later stages.
pub(crate) use crate::cli as entry;
pub(crate) use attachments as files;
pub(crate) use coordinator as runtime;
#[cfg(test)]
pub(crate) use native as transport;
pub(crate) use orchestration as orchestrator;
pub(crate) use prefs::{colors, settings};
pub(crate) use provider_glue::{callbacks, launch, resume_launch, usage};
pub(crate) use storage::io;
#[cfg(test)]
pub(crate) use storage::state_store as store;
