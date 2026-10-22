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
                owned_branches: room.github_branches.clone(),
                agents: self
                    .agents
                    .values()
                    .filter(|a| {
                        !a.deletion_pending
                            && (a.room_id == room.id || a.orchestrates == Some(room.id))
                    })
                    .map(|a| {
                        (
                            a.cwd.clone(),
                            if a.room_id == room.id {
                                a.branch.clone()
                            } else {
                                None
                            },
                        )
                    })
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
                    self.remember_github_heads(repo, scopes);
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
                        for scope in &scopes {
                            self.github_unavailable
                                .remove(&(RoomId(scope.room), repo.clone()));
                        }
                        let mark = self.github_watch.entry(repo.clone()).or_default();
                        let notices = github::changes(repo, mark, page, &scopes, *at_ms);
                        self.remember_github_heads(repo, &scopes);
                        for notice in notices {
                            tracing::debug!(
                                event = "bus.github.notice.created",
                                repo,
                                room_id = notice.room,
                                actionable = notice.actionable,
                                "GitHub notice created"
                            );
                            self.github_notice(
                                RoomId(notice.room),
                                notice.actionable,
                                notice.text,
                                now,
                            )?;
                        }
                    }
                    Err(failure) => {
                        for scope in scopes {
                            let room = RoomId(scope.room);
                            if self.github_unavailable.insert((room, repo.clone())) {
                                self.github_notice(room,true,format!("GitHub: {repo} watch unavailable ({}) — retrying in the background. Check gh installation / gh auth login.",failure.reason),now)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn remember_github_heads(&mut self, repo: &str, scopes: &[github::Scope]) {
        let mark = self.github_watch.get(repo);
        for scope in scopes {
            let Some(room) = self.rooms.get_mut(&RoomId(scope.room)).filter(|r| {
                !r.deletion_pending
                    && r.kind == RoomKind::Work
                    && r.github_repos
                        .as_ref()
                        .is_none_or(|repos| repos.iter().any(|r| r == repo))
            }) else {
                continue;
            };
            let branches = room.github_branches.entry(repo.into()).or_default();
            branches.extend(scope.branches.iter().cloned());
            branches.retain(|branch| mark.is_none_or(|m| !m.closed_head(branch)));
            if room.github_repos.is_none() {
                room.github_discovered.insert(repo.into());
            }
        }
    }

    fn github_notice(
        &mut self,
        room: RoomId,
        actionable: bool,
        text: String,
        now: u64,
    ) -> Result<(), ModelError> {
        let orchestrator = actionable
            .then(|| self.notice_orchestrator(room, None))
            .flatten();
        self.post_bus_notice(room, orchestrator, text, now)
            .map(|_| ())
    }
}

#[cfg(test)]
#[path = "tests/github_test.rs"]
mod tests;
