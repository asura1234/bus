//! Request ownership supplied by Bus, with the existing native composer reader.
use super::{InputObservation, SubmissionInspector, TerminalRuntime};
use crate::agents::AgentKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct DetectionContext {
    pub(super) obligation: bool,
    kind: Option<AgentKind>,
    activity_at: Option<Instant>,
    inspector: Option<Box<dyn SubmissionInspector>>,
    pristine: Arc<AtomicBool>,
}

impl DetectionContext {
    pub(super) fn ready(
        &self,
        kind: Option<AgentKind>,
        screen: &str,
        cursor: Option<(u16, u16)>,
    ) -> Option<bool> {
        (self.kind == kind).then_some(())?;
        self.inspector.as_ref().map(|inspect| {
            inspect.observe_with_launch(screen, cursor, self.pristine.load(Ordering::Acquire))
                == InputObservation::Empty
        })
    }

    pub(super) fn working(&self, now: Instant) -> bool {
        self.activity_at.is_some_and(|at| {
            now.saturating_duration_since(at) < super::detection_policy::AGENT_PENDING_IDLE_CAP
        })
    }
}

impl TerminalRuntime {
    pub(crate) fn set_detection_context(
        &self,
        kind: AgentKind,
        obligation: bool,
        activity_age_ms: Option<u64>,
        inspector: Option<Box<dyn SubmissionInspector>>,
    ) {
        let mut context = self
            .detection_context
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *context = DetectionContext {
            obligation,
            kind: Some(kind),
            activity_at: activity_age_ms
                .and_then(|age| Instant::now().checked_sub(Duration::from_millis(age))),
            inspector,
            pristine: self.startup_suggestion_open.clone(),
        };
    }

    pub(crate) fn clear_detection_context(&self) {
        *self
            .detection_context
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = DetectionContext::default();
    }
}
