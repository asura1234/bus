//! One restart outcome notice for each work room's orchestrator.
use super::{BusState, RoomKind};

impl BusState {
    /// Posts `text` to every live work room's orchestrator. Rooms without one
    /// are skipped. `Ok(false)` means there was nothing to deliver. A post
    /// error leaves the caller holding the outcome so it can retry.
    pub(crate) fn announce_restart(&mut self, text: &str, now_ms: u64) -> Result<bool, String> {
        let rooms: Vec<_> = self
            .rooms()
            .filter(|room| room.kind == RoomKind::Work && !room.deletion_pending)
            .map(|room| room.id)
            .collect();
        let mut posted = false;
        for room in rooms {
            let Some(orchestrator) = self.notice_orchestrator(room, None) else {
                continue;
            };
            self.post_bus_notice(room, Some(orchestrator), text.to_owned(), now_ms)
                .map_err(|error| error.to_string())?;
            posted = true;
        }
        Ok(posted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::model::{Author, Provider};

    #[test]
    fn restart_notice_reaches_each_work_room_orchestrator() {
        let mut state = BusState::default();
        let master = state.ensure_master_room();
        let alpha = state.create_room("alpha").unwrap();
        let beta = state.create_room("beta").unwrap();
        let quiet = state.create_room("quiet").unwrap();
        let alpha_orch = state
            .create_agent(
                master,
                "alpha-orch",
                Provider::ClaudeCode,
                "/repo".into(),
                None,
            )
            .unwrap();
        let beta_orch = state
            .create_agent(
                master,
                "beta-orch",
                Provider::ClaudeCode,
                "/repo".into(),
                None,
            )
            .unwrap();
        state.bind_orchestrator(alpha_orch, alpha).unwrap();
        state.bind_orchestrator(beta_orch, beta).unwrap();
        let text = "Restarted old -> new (by agent:9).";
        assert!(state.announce_restart(text, 10).unwrap());
        let texts = |agent| {
            state
                .requests()
                .filter(|request| request.agent_id == agent)
                .map(|request| request.prompt.text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(alpha_orch), vec![text.to_owned()]);
        assert_eq!(texts(beta_orch), vec![text.to_owned()]);
        assert!(state.requests().all(|request| request.room_id != quiet));
        assert!(state.requests().all(|request| request.room_id != master));
        assert!(state
            .requests()
            .all(|request| request.prompt.author == Author::Bus));
    }
}
