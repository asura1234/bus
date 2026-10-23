# Agent handover: <agent> → <replacement>

Date: <YYYY-MM-DD>
Room / task: <room and task goal>
Provider / role: <provider and current role>
PWD: <absolute working directory to use for the fresh agent>
Compaction notice: <bus → orchestrator: AGENT reached N compactions>

<!--
Write this note to temp/handover/<agent>-<date>.md in your PWD and report its
absolute path. Replace every placeholder; write "none" explicitly where empty.
The orchestrator already has broad-stroke context of what's been done and
what's next, so keep those sections to bullets with one sentence per point.
What's ongoing now must preserve the details that only you know.
-->

## What's been done previously

- <One sentence glossing over a completed result, with an evidence link if useful.>

## What's ongoing now

<!-- Be very detailed: a fresh agent must be able to resume without guessing. -->

**Exact current step and ownership:** <Where you stopped in the workflow,
the immediate objective, acceptance criteria, scope boundaries, and who owns
any shared files or related work.>

**PR peers and replacements:** <For each PR, give owner, verifier and every
reviewer by name/ID, current head and pending verdicts; name shared-file owners
and outstanding grants. The orchestrator updates the room-notes roster and
affected briefs and announces your replacement to these peers before it resumes.>

**Running or half-done work:** <Describe unfinished edits, partial results,
background processes, and why each is incomplete; distinguish completed
commands from commands still in flight. Say whether you are at a safe
checkpoint and what must finish before another agent can act.>

**Files touched:** <For every touched file, give the worktree, path, purpose,
what changed, and remaining edits; identify staged, unstaged and untracked
files and cross-reference their classification in Repository state.>

**Commands in flight:** <Exact commands and their working directories,
process/session IDs, status, output or log paths, how to inspect or wait for
them, and any cleanup or side effects; write "none" if nothing is running.>

**Evidence and current results:** <Relevant PRs, review lanes and verdicts,
test reports and the commit they prove, failures with exact errors, and
reproducible commands; distinguish verified facts from hypotheses.>

**Open questions and blockers:** <What is unclear, what you tried, the
decision needed, who can answer, and which work is waiting for an answer.>

**Gotchas and decisions:** <Constraints, rejected approaches and why,
environment details needed to resume, dependencies on other agents, and
anything that would be easy to repeat incorrectly or accidentally overwrite.>

## What's next

- <One sentence glossing over the next action, in execution order.>

## Repository state

Record every worktree and branch involved in the task, even if clean; include
branches not currently checked out as additional rows and mark that fact.
Capture current full commit hashes, not just remote tips or shortened hashes.

| Worktree (absolute path) | Branch (or detached HEAD; mark branches not checked out) | Current full commit hash |
|---|---|---|
| <path> | <branch> | <40-character SHA> |

Verify this snapshot with `git worktree list`, `git -C <worktree> branch -v`,
`git -C <worktree> rev-parse <branch-or-HEAD>`, and
`git -C <worktree> status --short --untracked-files=all` for each worktree.
Record every staged, unstaged and untracked change below, including ownership;
split a file into separate entries if its changes have different purposes.
For a clean worktree, explicitly write "none" in the change column.

| Worktree | Uncommitted change (path / hunk and status, or none) | Owner and purpose | Classification |
|---|---|---|---|
| <path> | <change> | <owner; why it exists> | <choose exactly one of the two labels below; n/a only for none> |

Use exactly **part of the task: commit-and-push when done** or
**exploratory: safe to stash** for each actual change. Do not stash changes as
part of writing this note; preserve them for the replacement and do not claim
another worker's changes are safe to stash without checking with their owner.

## Worker brief messaging rule

Bus automatically adds `name (agent id, room R):` to agent-authored messages. Send only the body; do not add a "from X", "X here" or signature line.

For repository work, the agent/harness reports meaningful status transitions:
PR/merge-request merged, ready for review, CI finished, a new conflict, or a
relevant upstream merge affecting the assignment. Include repo/PR/head/status
and next action. Message the room orchestrator; if none exists, post to You in
your own room with `bus send --room ROOM --as WORKER --to human --text "STATUS"`.
Send one update per meaningful transition, with normal worker attribution;
do not broadcast unrelated repository activity. See the repository-status rule
in the installed `parallel-writes.md` and `how-to-bus-cli.md` guides.

Use `--as YOUR_AGENT_ID` when messaging; `--to orchestrator` reaches your home
room's bound orchestrator, and names or IDs can reach agents in any room.

PR peers: `<PR/task> → owner <name/ID> → verifier <name/ID> → reviewer(s) <name/ID>`.
Use these peers from your brief. If one is missing or replaced, message the
orchestrator; it owns staffing, room notes, the roster and verifier allocation.
Workers do not edit room notes or the roster. The brief carries the exact head,
worktree, acceptance checklist, locked goal/non-goals, pinned PR base,
inherited `fix_round`, recorded merge authority and explicit holds.

Message peers directly. The verifier sends `VERIFIED <PR/task> @<full SHA>`
with evidence, or defects with `file:line` and reproduction, straight to the
owner (the PR author). Use the task key and worktree head before a PR exists.
Reviewers send verdicts and findings to the owner. The owner re-sends the full
head to verifier and reviewers on every push and gets current-head evidence.

If you own a PR, use the owner-run path in `auto-merge-pr` and enforce these
checks before every repair batch and merge, including inside `merge-pr`:

- Carry the inherited fix-round counter and record each batch in the PR repair
  log. Allow at most **2 fix rounds** across review, verification, gates, bots
  and developer feedback. If another repair is needed after round 2, stop and
  send a final with the count, cause and plan to the orchestrator.
- Never fix a finding outside the PR's locked goal in this PR. Exclude it and
  report it in the final as a follow-up for the orchestrator to decide and record.
- Re-measure added plus removed lines against the pinned PR base, excluding
  `plans/`, after each push and **before merging**. At **3000 lines**, stop
  and send a final to the orchestrator for splitting; do not merge.
- Hand back **anything FLAGGED or needing a developer decision** to the
  orchestrator in a final. Stop this PR's merge path until it is resolved;
  owners do not adjudicate those items themselves.
- Merge only with exact-head `VERIFIED`, every required reviewer Ready, gates
  and requested acceptance passing, comments adjudicated, recorded authority,
  no unresolved FLAG/decision and no explicit hold. This does not extend a
  workflow that ends at PR creation or waive a draft-only hold.

Before editing shared files, message their owners and read the installed
[Parallel writes](../parallel-writes.md) at the resolved absolute session path
and ownership map supplied in your brief. Finals still go to the orchestrator;
after peer-started work, also send the milestone report to `--to orchestrator`.
Staffing and recovery, dialogs, product decisions and replacement announcements
stay with the orchestrator. It updates the roster/briefs and tells affected
peers the replacement's name/ID before work resumes.
