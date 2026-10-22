//! Apply observations and their dedupe cursor in the same saved state.
use super::{BusState, ModelError, RoomId, RoomKind};
use crate::messaging::github::{self, Interest, Update, Watermark};
use std::collections::BTreeMap;

impl BusState {
    pub(crate) fn set_github_repos(
        &mut self,
        id: RoomId,
        value: &serde_json::Value,
    ) -> Result<(), String> {
        let repos = if value.is_null() {
            None
        } else {
            let values = value
                .as_array()
                .ok_or("repos must be null or an array of GitHub repositories")?;
            let mut repos = std::collections::BTreeSet::new();
            for value in values {
                let repo = value
                    .as_str()
                    .and_then(github::repository)
                    .ok_or("Invalid GitHub repository; use OWNER/REPO")?;
                repos.insert(repo);
            }
            Some(repos.into_iter().collect())
        };
        let room = self.rooms.get_mut(&id).ok_or("Unknown room")?;
        if room.kind != RoomKind::Work {
            return Err("GitHub watches belong to work rooms".into());
        }
        room.github_repos = repos;
        Ok(())
    }
    pub(crate) fn github_interests(&self) -> Vec<Interest> {
        self.rooms
            .values()
            .filter(|r| r.kind == RoomKind::Work && !r.deletion_pending)
            .map(|room| Interest {
                room: room.id.0,
                repos: room.github_repos.clone(),
                remembered: room.github_discovered.clone(),
                agents: self
                    .agents
                    .values()
                    .filter(|a| {
                        !a.deletion_pending
                            && (a.room_id == room.id || a.orchestrates == Some(room.id))
                    })
                    .map(|a| (a.cwd.clone(), a.branch.clone()))
                    .collect(),
            })
            .collect()
    }
    pub(crate) fn github_watermarks(&self) -> &BTreeMap<String, Watermark> {
        &self.github_watch
    }
    pub(crate) fn apply_github(&mut self, update: &Update, now: u64) -> Result<(), ModelError> {
        match update {
            Update::Coverage(coverage) => {
                for (repo, scopes) in coverage {
                    for scope in scopes {
                        if let Some(room) = self.rooms.get_mut(&RoomId(scope.room)) {
                            if room.github_repos.is_none() {
                                room.github_discovered.insert(repo.clone());
                            }
                        }
                    }
                }
            }
            Update::Repo {
                repo,
                scopes,
                result,
                at_ms,
            } => {
                let scopes: Vec<_> = scopes
                    .iter()
                    .filter(|scope| {
                        self.rooms.get(&RoomId(scope.room)).is_some_and(|r| {
                            !r.deletion_pending
                                && r.kind == RoomKind::Work
                                && r.github_repos
                                    .as_ref()
                                    .is_none_or(|repos| repos.contains(repo))
                        })
                    })
                    .cloned()
                    .collect();
                match result {
                    Ok(page) => {
                        let mark = self.github_watch.entry(repo.clone()).or_default();
                        let notices = github::changes(repo, mark, page, &scopes, *at_ms);
                        for notice in notices {
                            tracing::debug!(
                                event = "bus.github.notice.created",
                                repo,
                                room_id = notice.room,
                                actionable = notice.actionable,
                                "GitHub notice created"
                            );
                            self.github_notice(RoomId(notice.room), notice.text, now)?;
                        }
                    }
                    Err(failure) => {
                        for scope in scopes {
                            let room = RoomId(scope.room);
                            if self.github_unavailable.insert(room) {
                                self.github_notice(room,format!("GitHub: watch unavailable ({}) — retrying in the background. Check gh installation / gh auth login.",failure.reason),now)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn github_notice(&mut self, room: RoomId, text: String, now: u64) -> Result<(), ModelError> {
        let orchestrator = self.notice_orchestrator(room, None);
        let text = orchestrator.map_or_else(|| text.clone(), |id| format!("{text} @{}", id.0));
        self.post_bus_notice(room, orchestrator, text, now)
            .map(|_| ())
    }
}

#[cfg(test)]
#[path = "tests/github_test.rs"]
mod tests;
