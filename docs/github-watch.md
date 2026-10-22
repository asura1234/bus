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
`github_discovered`, plus remembered `github_branches` per repository. Overrides
are saved with the room. MASTER itself has no watch.

A PR belongs to the rooms whose current or remembered agent feature branches match
its head branch. Bus persists these branches per room and repository when discovery
sees them, so worker cleanup or switching branches does not lose ownership. An
unmatched head is informational; author identity never establishes ownership. The
repository's default branch does not establish ownership. With an explicit repo
list, the room's current agent branches apply to each listed repository.

Grey `bus` messages address the room's orchestrator for owned default-branch merges,
owned PRs becoming ready for review, completed CI (success or failure), and newly
appearing merge conflicts. Other default-branch merges are combined into one
informational message per repository per poll, with no recipient or orchestrator wake.
The existing `bus → orchestrator` header identifies actionable notices, without a
mention suffix. Bus uses its existing notice and message delivery; this feature does not add an idle digest.

A fresh watch reports default-branch merges from the previous 24 hours. Each
room's first observation of an owned open PR quietly records a baseline; later
polls report ready/CI/conflict transitions. Merge times use the local clock, with
a date for merges from earlier days. Notice text includes the repository, PR
number and head branch so the orchestrator can decide on cleanup, verification or follow-up.

The watcher runs outside the coordinator tick. It makes one GraphQL request per
repository per 90-second round, shared across rooms, with open and merged PR pages
advanced in subsequent rounds. Long scans therefore take more than one round.
GitHub rate-limit resets and retry-after headers delay retries; other failures
back off up to 24 minutes. Missing gh, authentication problems and unavailable
repositories are logged, with one unavailable notice per room/repository pair per
outage. Only a successful poll of that repository clears its flag; another
healthy repository cannot rearm the same outage. A later outage is reported again.
No raw CLI error output is posted to rooms.

Observation cursors and their notices are saved together in `state.json`. Restart
resumes pagination and suppresses already reported merges and PR-state edges.
The initial merge-time eligibility floor is fixed; the moving scan boundary uses
updatedAt, so a merge that moves pages remains eligible on the next scan. Seen
merge IDs remain persisted to avoid replay when older PRs receive new comments.
Open-PR observations are retained across page gaps; a confirmed merged PR removes
its open observation. Absence from a page is not treated as proof of closure.
CI completion uses GitHub's full state counts when checks span more than one page,
so a failure does not conceal other jobs still running.

GitHub API references: [GraphQL rate limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api)
and [gh api](https://cli.github.com/manual/gh_api).

The TUI driver's fake-provider sessions disable GitHub by default, so automated
TUI tests never contact GitHub. For an explicit read-only acceptance check only,
create `github-watch-live` inside that scratch session's data directory and make
its isolated home able to read your existing gh authentication. Production
sessions do not need this marker.
