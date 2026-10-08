pub(crate) mod attachments;
pub(crate) mod control;
pub(crate) mod coordinator;
pub(crate) mod diagnostics;
pub(crate) mod model;
pub(crate) mod native;
pub(crate) mod orchestration;
pub(crate) mod prefs;

pub(crate) mod storage;

// Temporary names for consumers moved in later stages.
pub(crate) use crate::cli as entry;
pub(crate) use attachments as files;
pub(crate) use coordinator as runtime;
#[cfg(test)]
pub(crate) use native as transport;
pub(crate) use orchestration as orchestrator;
pub(crate) use prefs::{colors, settings};
pub(crate) mod identity;
#[cfg(test)]
pub(crate) use crate::agents::providers::spool as callbacks;
pub(crate) use coordinator::usage;
pub(crate) mod launch {
    pub(crate) use super::coordinator::AddAgent;
    pub(crate) use crate::agents::providers::{
        launch::{provider_kind, SetupNotice},
        suggest::{suggestions, PathSuggestion},
    };
}

pub(crate) use storage::io;
#[cfg(test)]
pub(crate) use storage::state_store as store;

#[cfg(test)]
pub(crate) mod provider_glue {
    pub(crate) mod launch {
        mod tests {
            include!("../agents/providers/tests/launch_test.rs");
        }
    }
    pub(crate) mod callbacks {
        include!("../agents/providers/tests/spool_test.rs");
    }
    mod resume_launch {
        mod tests {
            include!("coordinator/resume/tests/capture_test.rs");
        }
    }
}
