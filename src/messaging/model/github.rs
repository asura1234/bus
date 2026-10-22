//! Apply observations and their dedupe cursor in the same saved state.
use super::{BusState, ModelError, RoomId, RoomKind};
use crate::messaging::github::{self, Interest, Update, Watermark};
use std::collections::{BTreeMap, BTreeSet};

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
                retired_branches: room.github_retired_branches.clone(),
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
                        self.remember_github_heads(repo, &scopes);
                        let scopes = self.github_open_scopes(repo, &scopes, page);
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
            let members: BTreeSet<_> = self
                .agents
                .values()
                .filter(|a| !a.deletion_pending && a.room_id == RoomId(scope.room))
                .filter_map(|a| a.branch.clone())
                .collect();
            let excluded = self.github_orchestrator_only_heads(RoomId(scope.room), repo, &members);
            let Some(room) = self.rooms.get_mut(&RoomId(scope.room)).filter(|r| {
                !r.deletion_pending
                    && r.kind == RoomKind::Work
                    && r.github_repos
                        .as_ref()
                        .is_none_or(|repos| repos.iter().any(|r| r == repo))
            }) else {
                continue;
            };
            room.github_member_branches
                .entry(repo.into())
                .or_default()
                .extend(scope.branches.intersection(&members).cloned());
            let branches = room.github_branches.entry(repo.into()).or_default();
            let retired = room.github_retired_branches.entry(repo.into()).or_default();
            retired.retain(|branch| {
                if excluded.contains(branch) {
                    false
                } else if mark.is_none_or(|m| !m.closed_head(branch)) {
                    branches.insert(branch.clone());
                    false
                } else {
                    true
                }
            });
            branches.extend(
                scope
                    .branches
                    .iter()
                    .filter(|branch| !retired.contains(*branch))
                    .cloned(),
            );
            branches.retain(|branch| {
                if excluded.contains(branch) {
                    false
                } else if mark.is_some_and(|m| m.can_retire_head(branch)) {
                    retired.insert(branch.clone());
                    false
                } else {
                    true
                }
            });
            if room.github_repos.is_none() {
                room.github_discovered.insert(repo.into());
            }
        }
    }

    fn github_orchestrator_only_heads(
        &self,
        room: RoomId,
        repo: &str,
        members: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let attributed = self
            .rooms
            .get(&room)
            .and_then(|r| r.github_member_branches.get(repo));
        let mark = self.github_watch.get(repo);
        self.agents
            .values()
            .filter(|a| !a.deletion_pending && a.orchestrates == Some(room))
            .filter_map(|a| a.branch.as_ref())
            .filter(|branch| !members.contains(*branch))
            .filter(|branch| attributed.is_none_or(|heads| !heads.contains(*branch)))
            .filter(|branch| {
                !mark.is_some_and(|m| {
                    m.head_prs
                        .get(*branch)
                        .is_some_and(|prs| prs.values().any(|closed| !closed))
                })
            })
            .cloned()
            .collect()
    }

    fn github_open_scopes(
        &self,
        repo: &str,
        scopes: &[github::Scope],
        page: &github::Page,
    ) -> Vec<github::Scope> {
        scopes
            .iter()
            .filter_map(|scope| {
                let room = self.rooms.get(&RoomId(scope.room))?;
                let mut branches = room.github_branches.get(repo).cloned().unwrap_or_default();
                if let Some(retired) = room.github_retired_branches.get(repo) {
                    branches.extend(
                        page.open
                            .iter()
                            .filter(|pr| retired.contains(&pr.branch))
                            .map(|pr| pr.branch.clone()),
                    );
                }
                Some(github::Scope {
                    room: scope.room,
                    branches,
                })
            })
            .collect()
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
