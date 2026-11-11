//! One directory-check batch in flight; filesystem latency never holds a poll.
use super::{mpsc, AgentId, BusState, Path, PathBuf};
use std::{collections::BTreeSet, io, sync::Arc};

type Source = Arc<dyn Fn(&Path) -> io::Result<bool> + Send + Sync>;
type Observation = (AgentId, PathBuf, Option<String>, io::Result<bool>);

pub(super) struct DirectoryChecks {
    source: Source,
    pending: Option<mpsc::Receiver<Vec<Observation>>>,
    invalidated: BTreeSet<AgentId>,
}

impl Default for DirectoryChecks {
    fn default() -> Self {
        Self {
            source: Arc::new(|path| std::fs::metadata(path).map(|metadata| metadata.is_dir())),
            pending: None,
            invalidated: BTreeSet::new(),
        }
    }
}

impl DirectoryChecks {
    /// A fresh check supersedes this recipient's in-flight observation, even
    /// when the replacement warning has exactly the same text.
    fn invalidate(&mut self, id: AgentId) {
        if self.pending.is_some() {
            self.invalidated.insert(id);
        }
    }

    pub(super) fn poll(&mut self, state: &mut BusState) -> Result<(), String> {
        if let Some(receiver) = &self.pending {
            let observations = match receiver.try_recv() {
                Ok(observations) => observations,
                Err(mpsc::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::TryRecvError::Disconnected) => Vec::new(),
            };
            self.pending = None;
            let invalidated = std::mem::take(&mut self.invalidated);
            for (id, cwd, warning, result) in observations {
                let Some(agent) = state.agent(id) else {
                    continue;
                };
                if invalidated.contains(&id)
                    || agent.deletion_pending
                    || agent.cwd != cwd
                    || agent.working_directory_error != warning
                {
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
                if unavailable {
                    record_missing_directory(state, id, &cwd)?;
                } else {
                    state
                        .observe_working_directory_error(id, None)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        let paths: Vec<_> = state
            .agents()
            .filter(|agent| !agent.deletion_pending)
            .map(|agent| {
                (
                    agent.id,
                    agent.cwd.clone(),
                    agent.working_directory_error.clone(),
                )
            })
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
                    .map(|(id, cwd, warning)| {
                        let result = source(&cwd);
                        (id, cwd, warning, result)
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

fn directory_removed(result: io::Result<std::fs::Metadata>) -> bool {
    match result {
        Ok(metadata) => !metadata.is_dir(),
        Err(error) => matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
        ),
    }
}

fn record_missing_directory(
    state: &mut BusState,
    id: AgentId,
    cwd: &Path,
) -> Result<String, String> {
    let error = format!(
        "Agent launch directory removed: {}; replace the agent with an existing --pwd.",
        cwd.display()
    );
    let changed = state
        .fail_missing_launch_directory(id, error.clone(), super::io::now_ms())
        .map_err(|e| e.to_string())?;
    if changed {
        tracing::warn!(event = "bus.agent.launch_directory_removed", agent_id = id.0,
            path = %cwd.display(), reason = "launch_directory_removed", %error,
            "Launch directory is gone; refusing further delivery");
    }
    Ok(error)
}

impl super::Worker {
    /// Admission and failure recovery need fresh evidence, before the next poll.
    /// Permission and other indeterminate filesystem errors do not prove removal.
    pub(super) fn check_launch_directory(&mut self, id: AgentId) -> Result<Option<String>, String> {
        let Some(agent) = self.state.agent(id) else {
            return Ok(None);
        };
        let cwd = agent.cwd.clone();
        if agent.deletion_pending {
            return Ok(None);
        }
        let result = std::fs::metadata(&cwd);
        if result.as_ref().is_ok_and(|metadata| metadata.is_dir()) {
            self.directory_checks.invalidate(id);
            if agent.working_directory_error.is_some() {
                let mut state = self.state.clone();
                state
                    .observe_working_directory_error(id, None)
                    .map_err(|e| e.to_string())?;
                self.save(state)?;
            }
            return Ok(None);
        }
        if !directory_removed(result) {
            return Ok(None);
        }
        self.directory_checks.invalidate(id);
        let mut state = self.state.clone();
        let error = record_missing_directory(&mut state, id, &cwd)?;
        if state != self.state {
            self.save(state)?;
        }
        Ok(Some(error))
    }

    pub(super) fn check_current_launch_directories(&mut self) -> Result<(), String> {
        let agents: Vec<_> = self
            .state
            .agents()
            .filter(|agent| agent.current_request.is_some())
            .map(|agent| agent.id)
            .collect();
        for agent in agents {
            self.check_launch_directory(agent)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/working_directory_sampler_test.rs"]
mod tests;
