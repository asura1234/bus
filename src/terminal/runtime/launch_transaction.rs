//! One shell-input transaction per managed launch. A prompt is a return witness
//! only after submission and only when the shell reports this attempt's ID.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaunchPhase {
    WaitingForPrompt,
    Verifying,
    Submitting,
    Submitted,
    Running,
    Returned,
    Failed,
}

#[derive(Debug)]
struct Attempt {
    id: u64,
    phase: LaunchPhase,
    prompt_cycle: u64,
    enter_attempted: bool,
    #[cfg(test)]
    input_confirmed: bool,
    error: Option<String>,
}

impl Attempt {
    fn matches_return(&self, witness: Option<(u64, u64)>) -> bool {
        let may_return = matches!(
            self.phase,
            LaunchPhase::Submitted | LaunchPhase::Running | LaunchPhase::Returned
        ) || (self.phase == LaunchPhase::Failed && self.enter_attempted);
        may_return && witness.is_some_and(|(cycle, id)| id == self.id && cycle > self.prompt_cycle)
    }
}

#[derive(Clone, Default)]
pub(super) struct LaunchTransaction(Arc<Mutex<Option<Attempt>>>);

impl LaunchTransaction {
    pub(super) fn begin(&self, pane: u32, prompt_cycle: u64) -> u64 {
        let id = NEXT_ATTEMPT.fetch_add(1, Ordering::Relaxed);
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(Attempt {
            id,
            phase: LaunchPhase::WaitingForPrompt,
            prompt_cycle,
            enter_attempted: false,
            #[cfg(test)]
            input_confirmed: false,
            error: None,
        });
        tracing::info!(event = "bus.agent.launch_phase", attempt_id = id, pane,
            phase = ?LaunchPhase::WaitingForPrompt, prompt_cycle);
        id
    }

    pub(super) fn transition(&self, id: u64, phase: LaunchPhase, cycle: u64) -> bool {
        let mut guard = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let Some(attempt) = guard.as_mut().filter(|attempt| attempt.id == id) else {
            return false;
        };
        attempt.phase = phase;
        attempt.prompt_cycle = cycle;
        attempt.enter_attempted |= phase == LaunchPhase::Submitting;
        #[cfg(test)]
        {
            attempt.input_confirmed |= phase == LaunchPhase::Submitted;
        }
        tracing::info!(
            event = "bus.agent.launch_phase",
            attempt_id = id,
            ?phase,
            prompt_cycle = cycle
        );
        true
    }

    pub(super) fn fail(&self, id: u64, error: String, witness: Option<(u64, u64)>) {
        let mut guard = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(attempt) = guard.as_mut().filter(|attempt| attempt.id == id) {
            tracing::warn!(event = "bus.agent.launch_input_failed", attempt_id = id,
                phase = ?LaunchPhase::Failed, previous_phase = ?attempt.phase,
                prompt_cycle = attempt.prompt_cycle,
                observed_prompt_cycle = ?witness.map(|(cycle, _)| cycle),
                observed_attempt_id = ?witness.map(|(_, id)| id),
                enter_attempted = attempt.enter_attempted, %error,
                "Launch input was not confirmed; retain the shell for inspection");
            attempt.phase = LaunchPhase::Failed;
            attempt.error = Some(error);
        }
    }

    pub(super) fn error(&self, definite_only: bool) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|attempt| !definite_only || !attempt.enter_attempted)
            .and_then(|attempt| attempt.error.clone())
    }

    pub(super) fn has_return_witness(&self, witness: Option<(u64, u64)>) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|attempt| attempt.matches_return(witness))
    }

    pub(super) fn returned(&self, witness: Option<(u64, u64)>, live_provider: bool) -> bool {
        let mut guard = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let Some(attempt) = guard.as_mut() else {
            return false;
        };
        if live_provider && attempt.phase == LaunchPhase::Submitted {
            attempt.phase = LaunchPhase::Running;
            tracing::info!(event = "bus.agent.launch_phase", attempt_id = attempt.id,
                phase = ?attempt.phase, prompt_cycle = attempt.prompt_cycle);
        }
        if live_provider || !attempt.matches_return(witness) {
            return false;
        }
        let Some((cycle, id)) = witness else {
            return false;
        };
        if attempt.phase != LaunchPhase::Returned {
            attempt.phase = LaunchPhase::Returned;
            tracing::info!(event = "bus.agent.launch_phase", attempt_id = id,
                phase = ?attempt.phase, prompt_cycle = attempt.prompt_cycle,
                observed_prompt_cycle = cycle);
        }
        true
    }

    pub(super) fn diagnostics(&self, witness: Option<(u64, u64)>) -> String {
        format!(
            "attempt={:?} observed_prompt={witness:?}",
            *self.0.lock().unwrap_or_else(|p| p.into_inner())
        )
    }

    #[cfg(test)]
    pub(super) fn input_confirmed(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|attempt| attempt.input_confirmed)
    }

    #[cfg(test)]
    pub(super) fn id(&self) -> Option<u64> {
        self.0.lock().unwrap().as_ref().map(|attempt| attempt.id)
    }
}

#[cfg(test)]
#[path = "tests/launch_transaction_test.rs"]
mod tests;
