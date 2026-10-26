//! One directory-check batch in flight; filesystem latency never holds a poll.
use super::{mpsc, AgentId, BusState, Path, PathBuf};
use std::{io, sync::Arc};

type Source = Arc<dyn Fn(&Path) -> io::Result<bool> + Send + Sync>;
type Observation = (AgentId, PathBuf, io::Result<bool>);

pub(super) struct DirectoryChecks {
    source: Source,
    pending: Option<mpsc::Receiver<Vec<Observation>>>,
}

impl Default for DirectoryChecks {
    fn default() -> Self {
        Self {
            source: Arc::new(|path| std::fs::metadata(path).map(|metadata| metadata.is_dir())),
            pending: None,
        }
    }
}

impl DirectoryChecks {
    pub(super) fn poll(&mut self, state: &mut BusState) -> Result<(), String> {
        if let Some(receiver) = &self.pending {
            let observations = match receiver.try_recv() {
                Ok(observations) => observations,
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => Vec::new(),
            };
            self.pending = None;
            for (id, cwd, result) in observations {
                let Some(agent) = state.agent(id) else {
                    continue;
                };
                if agent.deletion_pending || agent.cwd != cwd {
                    continue;
                }
                let unavailable = match result {
                    Ok(is_directory) => !is_directory,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        true
                    }
                    // An unreadable path is not evidence of disappearance or
                    // recovery. Keep the last known directory warning.
                    Err(_) => continue,
                };
                let error = unavailable.then(|| format!(
                    "Working directory is unavailable: {}. Provider hooks may fail before Bus runs. Restore the worktree or remount its filesystem, or re-add the agent with an existing --pwd.",
                    cwd.display()
                ));
                state
                    .observe_working_directory_error(id, error)
                    .map_err(|e| e.to_string())?;
            }
        }
        let paths: Vec<_> = state
            .agents()
            .filter(|agent| !agent.deletion_pending)
            .map(|agent| (agent.id, agent.cwd.clone()))
            .collect();
        if paths.is_empty() {
            return Ok(());
        }
        let source = self.source.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("bus-working-directory".into())
            .spawn(move || {
                let observations = paths
                    .into_iter()
                    .map(|(id, cwd)| {
                        let result = source(&cwd);
                        (id, cwd, result)
                    })
                    .collect();
                let _ = sender.send(observations);
            }) {
            Ok(_) => self.pending = Some(receiver),
            Err(error) => {
                tracing::warn!(event = "bus.working_directory.check_failed", %error, "Could not start directory observation; retaining warning state")
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/working_directory_sampler_test.rs"]
mod tests;
