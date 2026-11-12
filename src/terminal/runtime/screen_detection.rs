//! Small screen facts published by the detector; list polling never reads VT cells.
use super::TerminalRuntime;
use crate::agents::AgentKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(super) struct ScreenDetectionCache {
    content_seq: Arc<AtomicU64>,
    latest: Mutex<Option<ScreenDetection>>,
}

impl ScreenDetectionCache {
    /// Reuse the detector's active-screen text and parse outside the publication
    /// lock; list reads copy only the completed facts.
    pub(super) fn observe(
        &self,
        terminal: &crate::terminal::emulator::PaneTerminal,
        agent: Option<AgentKind>,
        process_exited: bool,
    ) -> (String, bool) {
        let revision = self.content_seq.load(Ordering::Acquire);
        let content = terminal.detection_text();
        let coherent = revision.is_multiple_of(2)
            && revision == self.content_seq.load(Ordering::Acquire)
            && !terminal.synchronized_output_active();
        if agent.is_none() || process_exited {
            *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = None;
        } else if coherent {
            let snapshot = ScreenDetection::from_text(agent, revision, &content);
            if revision == self.content_seq.load(Ordering::Acquire) {
                *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = Some(snapshot);
            }
        }
        // Retry after a partial frame, even if the sync-end marker changes no text.
        let coherent = coherent && revision == self.content_seq.load(Ordering::Acquire);
        (content, coherent)
    }

    pub(super) fn new(content_seq: Arc<AtomicU64>) -> Self {
        Self {
            content_seq,
            latest: Mutex::new(None),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ScreenDetection {
    agent: Option<AgentKind>,
    revision: u64,
    pub(crate) compaction_banner: bool,
    pub(crate) provider_quota: bool,
    dialog: Option<String>,
    working_dialog: Option<String>,
}

impl ScreenDetection {
    pub(super) fn from_text(agent: Option<AgentKind>, revision: u64, text: &str) -> Self {
        Self {
            agent,
            revision,
            compaction_banner: crate::agents::compaction_banner::visible(
                agent.map(crate::agents::agent_label),
                text,
            ),
            provider_quota: agent == Some(AgentKind::Cursor)
                && crate::agents::providers::cursor::usage::is_restored_monthly_usage_limit(text),
            dialog: crate::terminal::driver::blocker_from_text(text, false),
            working_dialog: crate::terminal::driver::blocker_from_text(text, true),
        }
    }

    pub(crate) fn dialog(&self, working: bool) -> Option<String> {
        if working {
            self.working_dialog.clone()
        } else {
            self.dialog.clone()
        }
    }
}

impl TerminalRuntime {
    pub(crate) fn cached_screen_detection(
        &self,
        agent: Option<AgentKind>,
    ) -> Option<ScreenDetection> {
        let before = self.content_seq();
        if !before.is_multiple_of(2) || self.synchronized_output_active() {
            return None;
        }
        // Never wait for the detector, including during publication.
        let snapshot = self.screen_detection.latest.try_lock().ok()?.clone()?;
        (snapshot.agent == agent && snapshot.revision == before && before == self.content_seq())
            .then_some(snapshot)
    }
}
