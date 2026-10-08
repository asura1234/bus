# Orchestrator guide

This guide is for a coding agent (Claude Code, Codex, Cursor) acting as the
orchestrator of a Bus room. It builds on [How to use the Bus CLI](how-to-bus-cli.md);
read that first for the commands themselves. The binding rules are in
[Orchestrator rules](orchestrator-rules.md).

Code explains itself; use short inline comments to explain why it was built that way where it is not obvious, and agents read the code for the rest.

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
  bound to one room for its whole life. The human chats with all orchestrators in MASTER and
  can still step into any room, act as its orchestrator directly, or read any
  agent's terminal.

Bus does not supervise the orchestrator. Its rules are the instructions it was
given, this guide, and the room's workflow file.

## Getting started

The human adds you to MASTER together with your room, from the TUI (add an
agent while MASTER is open) or the CLI:

```sh
bus agent add --room MASTER --name claude-orch --provider claude \
  --pwd "$(pwd)" --orchestrates "$room_id"
```

Bus launches you with an orchestrator system prompt (editable in the TUI form,
or `--system-prompt`/`--system-prompt-file`) that names your room and points to
the Bus docs in `<BUS_DATA_DIR>/docs/`. Bus writes nothing into your PWD. A new
PWD may show the provider's "trust this folder" prompt on first launch; the
human answers it in your terminal. Your room is fixed for your whole life:
there is no reassignment, and deleting the room deletes you with it.

On your first turn:

1. Run `bus state` and find your room, its agents, and your own agent ID.
2. Read the room's notes and history: `bus history --room "$room_id"`.
3. Agree on the workflow with the human before kicking off (next section).

Send every message to the room's agents as yourself, so the history shows who
asked:

```sh
bus send --room "$room_id" --as claude-orch --to claude-dev --text "..."
```

### Move an existing session into MASTER

A conversation that already runs as Claude Code, Codex or Cursor can become an
orchestrator without losing its context. The human quits it, then adds it to
MASTER with its session ID in the launch args (`--resume SESSION_ID`, or
`resume SESSION_ID` for Codex) and its original PWD. Bus resumes that session,
binds it to the new agent and still delivers the orchestrator prompt; see
"Move an existing session into MASTER" in [How to use the Bus CLI](how-to-bus-cli.md).

## The workflow file

Each room has a workflow file, a living plan the orchestrator keeps. Follow
[workflow-create](workflow-create.md) (in the Bus docs folder) and start from
[the workflow template](templates/workflow-template.md) and the
[example workflows](workflows/): goal,
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

The room notes are the human's status board. They watch many rooms and must
never need to ask an orchestrator for a status update, so keeping the notes
current is a standing duty, not something done on request. Rewrite them on
every state change:

- a task assigned, finished or failed,
- an agent waiting at a dialog (permission, trust, question),
- a decision made, by you or the human.

Make anything waiting on the human easy to spot. The notes are free text and Bus
checks no structure; shape them however reads best for the room. One layout
that works, with short lines:

1. `Workflow: <path>`, the room's workflow file.
2. `WAITING ON YOU:` with one item per decision, keypress or approval the
   human must give, at the top; left out when nothing waits on the human.
3. `Now:` what is running, and which agent runs it.
4. `DONE / OPEN:` a checklist of the workflow's steps, `[x]` done, `[ ]` open.

```sh
bus room notes "$room_id" --text "Workflow: ~/bus-data/workflows/pr-123.md
WAITING ON YOU:
- Approve claude-dev's push of feat/gallery (bus agent dialog claude-dev)
- Pick a gallery list: native list or RN lib (both pass benchmarks)
Now: review round 2 (claude-review, codex-review)
DONE / OPEN:
[x] Spec agreed
[x] Implement gallery (claude-dev)
[x] CI green
[ ] Review until all ready (round 2 of 4)
[ ] Human regression test
[ ] Merge (human)"
```

`room notes` replaces the whole field, so always send the full board. Update it
when the state changes, not on every message.

## Coordinate parallel work

Decide how agents share the repository before sending parallel work:

- **Default: one shared branch.** Give each agent its file boundaries. For a
  few shared files, tell each agent involved: "When you need to modify file X,
  ask me first and wait." Grant one agent at a time.
- **Only when it is a must:** give an agent its own worktree and branch. Ask an
  agent to create it with the `worktree-new` skill, merge the branches in a
  planned order, and have an agent run `worktree-close` as soon as each one is
  merged.

Write the choice in the workflow's Coordination section.

## Choose agents and models

- **Use different models for review.** Reviewers on different providers catch
  different problems.
- **Save quota for review.** `bus state` shows 5-hour and weekly usage per
  provider, under `usage`. Bus collects Claude and Codex usage; Cursor, and
  stale or missing data, report `unknown`. As a rule of thumb, once a provider
  is below about 25% of its weekly allowance, stop giving it coding tasks and
  keep it for review. Unknown usage is unknown, not full.
- **Move work when a provider runs out.** If an agent hits a usage limit, add an
  agent on another provider (`bus agent add`) and hand the work over.
- **Watch context health.** `bus state` shows how many times each agent's
  context has been compacted. After 5 compactions, ask a worker to write a
  handover note to a file, run `bus agent clear` on it, and have it continue
  from that note. Clear a worker the same way before an unrelated task. This
  limit is for workers only: the orchestrator never clears itself and is never
  replaced for compactions, because it carries the context of the whole effort.

## The review loop

A typical PR review loop, run by the orchestrator:

1. Ask two or more reviewers on different providers to run `review-pr` on the
   branch.
2. Send their findings to the author agent to run `address-review-comments`.
3. If findings are flagged for a decision, ask every reviewer for a
   recommendation, form your own, and pick the best (best-of-N). If the
   recommendations disagree sharply, stop and ask the human.
4. Repeat until every reviewer says ready.
5. If the branch touches delivery, callbacks or launch, have an agent run
   `just e2e` (live round trips with every provider) and report its table.
6. Tell the human the branch is ready for their regression test before merge.

## Steer agents without waiting

Correct an agent the moment you notice a problem; do not wait for its turn to
end. `bus send` to a working agent types the message into its running turn, and
the turn's reply answers the original task and every correction together:
`bus wait` on any of those messages returns the same reply. Send with `--queue`
only for an unrelated task that should get a turn and a reply of its own.

## Unblock agents

- **Dialogs:** agents stop at permission, trust and question dialogs and
  other screens they cannot pass alone. The blocked worker sends you one
  message, "Blocked, needs help to continue.", the same for every blocker and
  once per episode; `bus wait` also stops early with
  `agent_waiting_on_dialog`. Look at its terminal: `bus agent dialog AGENT`
  shows the question or full command, the options and a fresh fingerprint
  (`bus agent read AGENT --source visible` for any other screen). Then `bus
  agent choose AGENT --option N --fingerprint F`, or for a free-text question
  `bus agent answer AGENT --text "..." --fingerprint F` (`--skip` instead of
  `--text`). Decide, answer, and check the reported `outcome`; nothing follows
  when the dialog closes.
  Ask the human before approving anything destructive or outward-facing.
- **Stuck requests:** check `bus message status`, `bus diagnostics` and
  `bus agent read` before `bus request recover`.
- **Delivery is not completion.** Wait for `complete: true` from `bus wait` or
  `bus message status`.

## When to ask the human

Ask before anything outside the room's goal or the workflow's decision rules:

- Publishing, merging, pushing to shared branches, or deleting work. Adding
  and deleting agents in your own room is not on this list: do it as the
  workflow needs and log it.
- Changing the goal or dropping a requirement.
- Reviewers or best-of-N candidates that disagree sharply.
- A hard gate that no option can meet.

If the human gave you full control for a decision, decide, record it in the
workflow log, and tell the human in MASTER what you decided.
