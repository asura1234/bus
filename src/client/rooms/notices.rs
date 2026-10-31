use super::BusUi;

impl BusUi {
    pub(super) fn visible_error(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| {
            self.snapshot
                .error
                .as_deref()
                .filter(|error| self.dismissed_snapshot_error.as_deref() != Some(*error))
        })
    }

    fn agent_notices(&self) -> Option<String> {
        let room = self.room?;
        let notices = self
            .snapshot
            .state
            .agents()
            .filter(|agent| agent.room_id == room)
            .filter_map(|agent| {
                agent
                    .actionable_error
                    .as_ref()
                    .map(|error| format!("{}: {error}", agent.name))
            })
            .collect::<Vec<_>>()
            .join(" · ");
        (!notices.is_empty()).then_some(notices)
    }

    pub(super) fn sync_agent_notices(&mut self) {
        let notices = self.agent_notices();
        if notices != self.observed_agent_notices {
            if let Some(notices) = &notices {
                tracing::info!(event = "bus.agent.notice", notices = %notices, "Agent notice");
            }
            self.observed_agent_notices = notices;
        }
    }

    pub(super) fn storage_is_paused(&self) -> bool {
        self.error
            .as_deref()
            .is_some_and(super::storage_paused_error)
            || self
                .snapshot
                .error
                .as_deref()
                .is_some_and(super::storage_paused_error)
    }
}
