# GitHub watch

Bus watches GitHub repositories covered by each work room. It reads GitHub using
`gh` and your existing `gh auth login`; it does not change PRs, checks, or branches.
Only github.com remotes are supported.

Repositories come from the git remotes of the room's agents, including its MASTER
orchestrator. Bus remembers discovered repositories so cleaning up the last worker
does not stop merge monitoring. Set an explicit list, return to discovery, or turn
monitoring off for a room:

```sh
bus room github ROOM --repos asura1234/bus,owner/other-repo
bus room github ROOM --auto
bus room github ROOM --off
```

`bus state` includes `github_repos` (null means automatic, [] means off) and
`github_discovered`. Overrides are saved with the room. MASTER itself has no watch.

A PR belongs to a room when its head branch matches an agent's feature branch, or
its author matches the GitHub author of that feature branch's remote tip. The
repository's default branch does not establish ownership. With an explicit repo
list, the room's agent branches apply to each listed repository.

Grey `bus` messages mention the room's orchestrator for default-branch merges,
owned PRs becoming ready for review, completed CI (success or failure), and newly
appearing merge conflicts. Other default-branch merges are combined into one
informational message per repository per poll. Bus uses its existing notice and
message delivery; this feature does not add an idle digest.

A fresh watch reports default-branch merges from the previous 24 hours and the
current ready/CI/conflict state of owned open PRs. Later polls report transitions.
Merge times are labelled UTC. Notice text includes the repository, PR number and
head branch so the orchestrator can decide on cleanup, verification or follow-up.

The watcher runs outside the coordinator tick. It makes one GraphQL request per
repository per 90-second round, shared across rooms, with open and merged PR pages
advanced in subsequent rounds. Long scans therefore take more than one round.
GitHub rate-limit resets and retry-after headers delay retries; other failures
back off up to 24 minutes. Missing gh, authentication problems and unavailable
repositories are logged, with one unavailable notice per room. No raw CLI error
output is posted to rooms.

Observation cursors and their notices are saved together in `state.json`. Restart
resumes pagination and suppresses already reported merges and PR-state edges.
CI completion uses GitHub's full state counts when checks span more than one page,
so a failure does not conceal other jobs still running.

GitHub API references: [GraphQL rate limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api)
and [gh api](https://cli.github.com/manual/gh_api).

The TUI driver's fake-provider sessions disable GitHub by default, so automated
TUI tests never contact GitHub. For an explicit read-only acceptance check only,
create `github-watch-live` inside that scratch session's data directory and make
its isolated home able to read your existing gh authentication. Production
sessions do not need this marker.
