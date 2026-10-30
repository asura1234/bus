//! One restart outcome notice per tick, then the helper's file is removed.
use super::Worker;

impl Worker {
    /// A failed save leaves the file so the next tick can try again.
    pub(super) fn announce_recorded_restart(&mut self) -> Result<(), String> {
        let path = crate::utils::restart_record::outcome_path(&self.data_dir);
        let Some(record) = crate::utils::restart_record::read_outcome(&path)? else {
            return Ok(());
        };
        let mut state = self.state.clone();
        let text = crate::utils::restart_record::notice_text(&record);
        // A post error must keep outcome.json. The next tick warns and retries.
        // Nothing to deliver (no live work-room orchestrator) may delete it.
        let posted = state.announce_restart(&text, crate::messaging::storage::io::now_ms())?;
        if posted {
            self.save(state)?;
        }
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
