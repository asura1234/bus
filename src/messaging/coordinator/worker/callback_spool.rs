//! Retention of rejected callback envelopes after their state is durable.
use super::{spool, AgentId, Path, PathBuf, Worker};

impl Worker {
    /// Invalid envelopes leave the active scan only once the agent error is saved.
    pub(in crate::messaging::coordinator) fn retain_rejected_spool(
        &mut self,
        id: AgentId,
        dir: &Path,
        path: PathBuf,
    ) -> Result<(), String> {
        if let Some(batch) = self.batch.as_mut() {
            batch.rejections.push((id, dir.to_path_buf(), path));
            return Ok(());
        }
        spool::retain_rejected(dir, &path, spool::RoutingKey(id.0)).map_err(|e| e.to_string())
    }
}
