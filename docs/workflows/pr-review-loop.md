# Workflow: pr-123 review loop

<!-- Example. A PR is written; reviewers on different models review it until
all of them say ready, then the human regression-tests it before merge. -->

## Goal

PR #123 is ready to merge: every reviewer says ready, CI is green, and the
human's regression test passes.

## Non-goals

- New features beyond the PR's scope. Reviewers file them as follow-ups.

## Graph

```mermaid
flowchart TD
    start([Human: get PR 123 merge-ready]):::human --> review

    subgraph round [Review round, at most 4]
        review[claude-review + codex-review + cursor-review: review-pr in parallel]
        review --> ready{All reviewers ready?}
        ready -- no --> flagged{Flagged items?}
        flagged -- yes --> bestof[All reviewers: recommend a fix for each flagged item]
        bestof --> pick{Recommendations agree?}
        pick -- yes --> fix
        pick -- no --> ask[Human: decide flagged items]:::human
        ask --> fix
        flagged -- no --> fix[author: address-review-comments, push]
        fix --> ci{CI green?}
        ci -- no --> fix
        ci -- yes --> review
    end

    ready -- yes --> regress[Human: regression test]:::human
    ready -- round 4 still not ready --> stuck[Human: cut scope or continue]:::human
    stuck --> review
    regress -- problem --> fix
    regress -- ok --> merge([Human: merge])

    classDef human fill:#fde2e4,stroke:#c9184a
    classDef current stroke-width:3px,stroke:#2563eb
    class review current
```

## Participants

| Agent | Provider | Role | Worktree / branch |
|---|---|---|---|
| author | claude | address-review-comments, push | main worktree, PR branch |
| claude-review | claude | review-pr | main worktree, read-only |
| codex-review | codex | review-pr | main worktree, read-only |
| cursor-review | cursor | review-pr | main worktree, read-only |

## Gates

- CI green on the pushed head before each review round.
- Every reviewer answers ready on the same head commit.

## Coordination

- Reviewers only read; the author is the only writer on the branch.
- Each round reviews the delta since the previous round (`review-pr` round 2+).

## Decision rules

- The orchestrator picks the fix for a flagged item when all recommendations
  agree; otherwise the human decides.
- Cap: 4 rounds. After that, ask the human to cut scope or allow more rounds.
- The human merges; the orchestrator never does.

## Log

- 2026-10-06: Drafted with the human.
