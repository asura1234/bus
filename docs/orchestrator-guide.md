# Orchestrator guide

This guide is for a coding agent (Claude Code, Codex, Cursor) acting as the
orchestrator of a Bus room. It builds on [How to use the Bus CLI](how-to-bus-cli.md);
read that first for the commands themselves.

## What an orchestrator is

A room is one unit of work: a PR, a feature, an investigation. The agents in the
room do the work. The orchestrator runs it.

- **It delegates everything.** The orchestrator does not write code, run
  builds, or review diffs itself. It sends that work to the room's agents and
  reads their replies. This keeps its own context free for the whole job, while
  each worker starts with fresh context.
- **It stays available.** The human can ask it a question or change direction at
  any time, and it passes that steering to the agents doing the work.
- **It lives in MASTER.** Orchestrators are agents in the MASTER room, each
  assigned to one room. The human chats with all orchestrators in MASTER and
  can still step into any room, act as its orchestrator directly, or read any
  agent's terminal.

Bus does not supervise the orchestrator. Its rules are the instructions it was
given, this guide, and the room's workflow file.

## Getting started

The human adds you to MASTER and assigns your room:

```sh
bus agent add --room MASTER --name claude-orch --provider claude \
  --pwd /path/to/repo --orchestrates "$room_id"
```

On your first turn:

1. Run `bus state` and find your room, its agents, and your own agent ID.
2. Read the room's notes and history: `bus history --room "$room_id"`.
3. Agree on the workflow with the human before kicking off (next section).

Send every message to the room's agents as yourself, so the history shows who
asked:

```sh
bus send --room "$room_id" --as claude-orch --to claude-dev --text "..."
```

## The workflow file

Each room has a `workflow.md`, a living plan the orchestrator keeps. Start from
[docs/templates/workflow-template.md](templates/workflow-template.md): goal,
non-goals, a mermaid graph of the steps, participants, gates, coordination
rules, decision rules, and a log.

- **Draft it with the human** before work starts. The graph is a reference, not
  a script.
- **Keep it where the human can open it.** Put its path on the first line of the
  room notes.
- **Rewrite it whenever it stops matching reality.** The usual reasons are a
  failure that needs a different route (every spike misses a hard benchmark
  gate, so go back to research or change technology), and the human changing
  the requirements mid-work. Pass the change to the agents it affects.
- **Log every change** with one line and the reason, newest first.
- **Mark the current node** with `:::current` so the human can see where the
  room is.

## Keep the room readable at a glance

The human watches many rooms. Keep the room notes short and current, so one
look tells them the state:

```sh
bus room notes "$room_id" --text "Workflow: .bus/workflows/pr-123.md
Now: review round 2 (claude-review, codex-review)
Waiting on: human regression test
Decided: dropped the RN gallery lib, using native list (benchmarks)"
```

Update it when the state changes, not on every message.

## Coordinate parallel work

Decide how agents share the repository before sending parallel work:

- **No overlap:** agents can share one branch and worktree.
- **Heavy overlap:** give each agent its own worktree and branch. Ask an agent
  to create it with the `worktree-new` skill and remove it later with
  `worktree-close`, then merge the branches in a planned order.
- **A few shared files:** keep one worktree and tell each agent involved: "When
  you need to modify file X, ask me first and wait." Grant one agent at a time.

Write the choice in the workflow's Coordination section.

## Choose agents and models

- **Use different models for review.** Reviewers on different providers catch
  different problems.
- **Save quota for review.** `bus state` shows each agent's 5-hour and weekly
  usage where Bus can read it. As a rule of thumb, once a provider is below
  about 25% of its weekly allowance, stop giving it coding tasks and keep it for
  review. Unknown usage is unknown, not full.
- **Move work when a provider runs out.** If an agent hits a usage limit, add an
  agent on another provider (`bus agent add`) and hand the work over.
- **Watch context health.** `bus state` shows how many times each agent's
  context has been compacted. After about 20 compactions, ask the agent to write
  a handover note to a file, then start a fresh agent from that note.

## The review loop

A typical PR review loop, run by the orchestrator:

1. Ask two or more reviewers on different providers to run `review-pr` on the
   branch.
2. Send their findings to the author agent to run `address-review-comments`.
3. If findings are flagged for a decision, ask every reviewer for a
   recommendation, form your own, and pick the best (best-of-N). If the
   recommendations disagree sharply, stop and ask the human.
4. Repeat until every reviewer says ready.
5. Tell the human the branch is ready for their regression test before merge.

## Unblock agents

- **Permission prompts:** `bus agent permission AGENT` shows the prompt's
  fingerprint; approve a safe one once with `bus agent approve-once`. Ask the
  human about anything destructive or outward-facing.
- **Stuck requests:** check `bus message status`, `bus diagnostics` and
  `bus agent read` before `bus request recover`.
- **Delivery is not completion.** Wait for `complete: true` from `bus wait` or
  `bus message status`.

## When to ask the human

Ask before anything outside the room's goal or the workflow's decision rules:

- Publishing, merging, pushing to shared branches, or deleting work.
- Changing the goal or dropping a requirement.
- Reviewers or best-of-N candidates that disagree sharply.
- A hard gate that no option can meet.

If the human gave you full control for a decision, decide, record it in the
workflow log, and tell the human in MASTER what you decided.
