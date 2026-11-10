//! Callback hints accelerate the existing durable, identity-fenced consumer.
use super::{AgentId, BusCommand, Worker};

impl Worker {
    pub(in crate::messaging::coordinator) fn coalesce_callback_wakes(
        &mut self,
        pending: &mut std::collections::VecDeque<(u64, BusCommand)>,
    ) {
        // Only adjacent hints may combine. In particular, never move callbacks
        // across an interrupt/delete/resume command already admitted ahead of them.
        while matches!(pending.front(), Some((_, BusCommand::CallbackReady(_)))) {
            if let Some((_, BusCommand::CallbackReady(launch))) = pending.pop_front() {
                self.queue_callback_wake(&launch);
            }
        }
    }

    pub(in crate::messaging::coordinator) fn queue_callback_wake(&mut self, launch: &str) {
        if let Some(agent) = self.state.agents().find(|a| {
            !a.deletion_pending && a.runtime_identity.launch_id.as_deref() == Some(launch)
        }) {
            self.callback_wakes.insert(agent.id);
        }
    }

    pub(in crate::messaging::coordinator) fn consume_woken_callbacks(
        &mut self,
    ) -> Result<(), String> {
        if self.storage_pause.is_some() || self.callback_wakes.is_empty() {
            return Ok(());
        }
        let ids: Vec<AgentId> = self.callback_wakes.iter().copied().collect();
        self.begin_batch();
        let consumed = (|| {
            for &id in &ids {
                // A callback can arrive before its writer receipt. Preserve the
                // same writer-before-callback order as the periodic consumer.
                if self.native_submissions.contains_key(&id) {
                    continue;
                }
                let agent = self.state.agent(id).cloned();
                self.callback_wakes.remove(&id);
                let Some(agent) = agent.filter(|a| !a.deletion_pending) else {
                    continue;
                };
                let Some(launch) = agent.runtime_identity.launch_id else {
                    continue;
                };
                let dir = self.data_dir.join("callbacks").join(launch);
                self.consume_callbacks(id, &dir)?;
                // A publisher holding append.lock may have left a durable stage.
                // Retry it promptly without waiting for the next full poll.
                if std::fs::read_dir(&dir)
                    .map_err(|e| e.to_string())?
                    .any(|e| {
                        e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("pending-"))
                    })
                {
                    self.callback_wakes.insert(id);
                }
            }
            Ok(())
        })();
        let finished = self.finish_batch();
        let result = consumed.and(finished);
        if result.is_err() {
            self.callback_wakes.extend(ids);
        }
        result
    }
}
