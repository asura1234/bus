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
Use `--as YOUR_AGENT_ID` when messaging; `--to orchestrator` reaches your home
room's bound orchestrator, and names or IDs can reach agents in any room.

PR peers: `<PR/task> → owner <name/ID> → verifier <name/ID> → reviewer(s) <name/ID>`.
Fill every peer before dispatch; list all required reviewers. Carry the exact
head, worktree, acceptance checklist and explicit holds in the brief. Keep this
roster in the room notes and update affected briefs whenever an agent is
replaced. Assign one verifier per 1–2 PRs, adding verifiers as the PR count grows.

Message these peers directly. The verifier sends `VERIFIED <PR> @<full SHA>`
with evidence, or defects with `file:line` and reproduction, straight to the
owner. Reviewers send their verdicts and findings to the owner. The owner
re-sends the full head SHA to verifier and reviewers on every push and gets
current-head evidence before merging through `auto-merge-pr` under recorded
authority and holds. This does not extend a workflow that ends at PR creation.

Before editing shared files, message their owners and follow
[Parallel writes](../parallel-writes.md); supply its resolved absolute session
path and the ownership map in each parallel brief. Finals still go to the
orchestrator; after peer-started work, also send the milestone report directly
to `--to orchestrator`. Staffing and recovery, dialogs, product decisions and
announcing replacements stay with the orchestrator, which updates the roster
and tells every affected peer the replacement's name/ID before work resumes.
