pub(crate) mod attachments;
pub(crate) mod control;
pub(crate) mod coordinator;
pub(crate) mod diagnostics;
pub(crate) mod model;
pub(crate) mod native;
pub(crate) mod orchestration;
pub(crate) mod prefs;

pub(crate) mod storage;

pub(crate) mod identity;

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
