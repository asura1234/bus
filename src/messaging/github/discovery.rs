use super::{events::remote_repository, process, Scope};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct Interest {
    pub(crate) room: u64,
    pub(crate) repos: Option<Vec<String>>,
    pub(crate) remembered: BTreeSet<String>,
    pub(crate) owned_branches: BTreeMap<String, BTreeSet<String>>,
    pub(crate) retired_branches: BTreeMap<String, BTreeSet<String>>,
    pub(crate) agents: Vec<(PathBuf, Option<String>)>,
}

pub(super) fn discover(interests: &[Interest]) -> BTreeMap<String, Vec<Scope>> {
    let mut remotes = BTreeMap::new();
    for interest in interests {
        for (cwd, _) in &interest.agents {
            remotes.entry(cwd).or_insert_with(|| {
                process::run(
                    "git",
                    &[
                        "-C".into(),
                        cwd.to_string_lossy().into(),
                        "config".into(),
                        "--get-regexp".into(),
                        "^remote\\..*\\.url$".into(),
                    ],
                )
                .ok()
                .map(|output| remote_repos(&output))
                .unwrap_or_default()
            });
        }
    }
    coverage(interests, &remotes)
}
fn remote_repos(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .filter_map(|(_, url)| remote_repository(url))
        .collect()
}
fn coverage(
    interests: &[Interest],
    remotes: &BTreeMap<&PathBuf, BTreeSet<String>>,
) -> BTreeMap<String, Vec<Scope>> {
    let mut result: BTreeMap<String, Vec<Scope>> = BTreeMap::new();
    for interest in interests {
        let mut rooms: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        if let Some(repos) = &interest.repos {
            for repo in repos {
                rooms.entry(repo.clone()).or_default();
            }
        } else {
            for repo in &interest.remembered {
                rooms.entry(repo.clone()).or_default();
            }
            for (cwd, _) in &interest.agents {
                for repo in remotes.get(cwd).into_iter().flatten() {
                    rooms.entry(repo.clone()).or_default();
                }
            }
        }
        for (repo, branches) in &mut rooms {
            branches.extend(
                interest
                    .owned_branches
                    .get(repo)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
        }
        for (cwd, branch) in &interest.agents {
            if let Some(branch) = branch {
                for (repo, branches) in &mut rooms {
                    // An override supplies repositories even for an agent without a remote.
                    if interest.repos.is_some()
                        || remotes.get(cwd).is_some_and(|set| set.contains(repo))
                    {
                        branches.insert(branch.clone());
                    }
                }
            }
        }
        for (repo, mut branches) in rooms {
            if let Some(retired) = interest.retired_branches.get(&repo) {
                branches.retain(|branch| !retired.contains(branch));
            }
            result.entry(repo).or_default().push(Scope {
                room: interest.room,
                branches,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_watch_discovers_shared_repos_ignores_non_github_and_obeys_off_override() {
        let cwd = PathBuf::from("/agent");
        let remotes=[(&cwd,remote_repos("remote.origin.url git@github.com:Owner/Repo.git\nremote.upstream.url https://gitlab.com/o/r\n"))].into();
        let a = Interest {
            room: 7,
            agents: vec![(cwd.clone(), Some("fix/a".into()))],
            ..Default::default()
        };
        let mut b = a.clone();
        b.room = 8;
        let result = coverage(&[a.clone(), b.clone()], &remotes);
        assert_eq!(result.len(), 1);
        assert_eq!(result["owner/repo"].len(), 2);
        b.repos = Some(vec![]);
        assert_eq!(coverage(&[a, b], &remotes)["owner/repo"].len(), 1);
    }
}

#[cfg(test)]
mod remembered_tests {
    use super::*;
    #[test]
    fn github_watch_remembered_branches_stay_in_their_repository_and_obey_overrides() {
        let mut interest = Interest {
            room: 7,
            remembered: ["o/one".into(), "o/two".into()].into(),
            owned_branches: [
                ("o/one".into(), ["fix/a".into()].into()),
                ("o/two".into(), ["fix/b".into()].into()),
            ]
            .into(),
            ..Default::default()
        };
        let found = coverage(&[interest.clone()], &BTreeMap::new());
        assert_eq!(found["o/one"][0].branches, ["fix/a".into()].into());
        assert_eq!(found["o/two"][0].branches, ["fix/b".into()].into());
        interest.repos = Some(vec!["o/two".into()]);
        let found = coverage(&[interest.clone()], &BTreeMap::new());
        assert_eq!(found.len(), 1);
        assert_eq!(found["o/two"][0].branches, ["fix/b".into()].into());
        interest.repos = Some(vec![]);
        assert!(coverage(&[interest], &BTreeMap::new()).is_empty());
    }

    #[test]
    fn github_watch_retired_heads_do_not_reenter_scopes_from_a_current_checkout() {
        let interest = Interest {
            room: 7,
            repos: Some(vec!["o/one".into(), "o/two".into()]),
            agents: vec![(PathBuf::from("/repo"), Some("fix/shared".into()))],
            retired_branches: [("o/one".into(), ["fix/shared".into()].into())].into(),
            ..Default::default()
        };
        let found = coverage(&[interest], &BTreeMap::new());
        assert!(found["o/one"][0].branches.is_empty());
        assert_eq!(found["o/two"][0].branches, ["fix/shared".into()].into());
    }
}
