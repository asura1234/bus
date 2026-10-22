# Orchestrator guide

This guide is for a coding agent (Claude Code, Codex, Cursor) acting as the
orchestrator of a Bus room. It builds on [How to use the Bus CLI](how-to-bus-cli.md);
read that first for the commands themselves. The binding rules are in
[Orchestrator rules](orchestrator-rules.md).

Start with [Never act on assumptions; ask the developer](orchestrator-rules.md#never-act-on-assumptions),
the first binding rule, before kickoff or steering workers.

Code explains itself; use short inline comments to explain why it was built that way where it is not obvious, and agents read the code for the rest.

## What an orchestrator is

A room is one unit of work: a PR, a feature, an investigation. The agents in the
room do the work. The orchestrator runs it.

- **It delegates everything.** The orchestrator does not write code, run
  builds, or review diffs itself. It sends that work to the room's agents and
  reads their replies. This keeps its own context free for the whole job, while
  each worker starts with fresh context.
- **It stays available.** The developer can ask it a question or change direction at
  any time, and it passes that steering to the agents doing the work.
- **It lives in MASTER.** Orchestrators are agents in the MASTER room, each
  bound to one room for its whole life. The developer chats with all orchestrators in MASTER and
  can still step into any room, act as its orchestrator directly, or read any
  agent's terminal.

Bus does not supervise the orchestrator. Its rules are the instructions it was
given, this guide, and the room's workflow file.

## Getting started

The developer adds you to MASTER together with your room, from the TUI (add an
agent while MASTER is open) or the CLI:

```sh
bus agent add --room MASTER --name claude-orch --provider claude \
  --pwd "$(pwd)" --orchestrates "$room_id"
```

Bus launches you with an orchestrator system prompt (editable in the TUI form,
or `--system-prompt`/`--system-prompt-file`) that names your room and points to
the Bus docs in `<BUS_DATA_DIR>/docs/`. Bus writes nothing into your PWD. A new
PWD may show the provider's "trust this folder" prompt on first launch; the
developer answers it in your terminal. Your room is fixed for your whole life:
there is no reassignment, and deleting the room deletes you with it.

On your first turn:

1. Run `bus state` and find your room, its agents, and your own agent ID.
2. Read the room's notes and history: `bus history --room "$room_id"`.
3. Agree on the workflow with the developer before kicking off (next section).

Send every message to the room's agents as yourself, so the history shows who
asked:

```sh
bus send --room "$room_id" --as claude-orch --to claude-dev --text "..."
```

### Move an existing session into MASTER

A conversation that already runs as Claude Code, Codex or Cursor can become an
orchestrator without losing its context. The developer quits it, then adds it to
MASTER with its session ID in the launch args (`--resume SESSION_ID`, or
`resume SESSION_ID` for Codex) and its original PWD. Bus resumes that session,
binds it to the new agent and still delivers the orchestrator prompt; see
"Move an existing session into MASTER" in [How to use the Bus CLI](how-to-bus-cli.md).

## The workflow file

Each room has a workflow file, a living plan the orchestrator keeps: goal,
non-goals, a mermaid graph of the steps, participants, gates, coordination
rules, decision rules, and a log. Follow
[workflow-create](workflow-create.md) (in the Bus docs folder) and start from
[the workflow template](templates/workflow-template.md). For workflow lookup,
the resolved directory and shipped names, follow the
[binding named-workflows rule](orchestrator-rules.md#named-workflows).

- **Draft it with the developer** before work starts. The graph is a reference, not
  a script.
- **Keep it where the developer can open it.** Put its path on the first line of the
  room notes.
- **Rewrite it whenever it stops matching reality.** The usual reasons are a
  failure that needs a different route (every spike misses a hard benchmark
  gate, so go back to research or change technology), and the developer changing
  the requirements mid-work. Pass the change to the agents it affects.
- **Log every change** with one line and the reason, newest first.
- **Mark the current node** with `:::current` so the developer can see where the
  room is.

## Keep the room readable at a glance

The room notes are the developer's status board. They watch many rooms and must
never need to ask an orchestrator for a status update, so keeping the notes
current is a standing duty, not something done on request. Rewrite them on
every state change:

- a task assigned, finished or failed,
- an agent waiting at a dialog (permission, trust, question),
- a decision made, by you or the developer.

Make anything waiting on the developer easy to spot. The notes are free text and Bus
checks no structure; shape them however reads best for the room. One layout
that works, with short lines:

1. `Workflow: <path>`, the room's workflow file.
2. `WAITING ON YOU:` with one item per decision, keypress or approval the
   developer must give, at the top; left out when nothing waits on the developer.
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
[ ] Review, verification and bots (fix round 2 of 2)
[ ] Developer regression test
[ ] Merge (developer)"
```

`room notes` replaces the whole field, so always send the full board. Update it
when the state changes, not on every message.

## Coordinate parallel work

Decide how agents share the repository before sending parallel work:

- Whenever a change spans several places, split it across parallel agents.
  Order dependencies explicitly. Strongly encourage workers to use subagents
  per acceptance item or group for both investigation and fixes.
- Follow [Parallel writes](parallel-writes.md), the shared guide for
  orchestrators and workers, for ownership maps, shared-file locks, worktrees,
  integration and cleanup. Default to one shared branch; switch to separate
  worktrees for heavy overlap or conflicting gates/builds.
- In **every parallel worker's brief**, include its ownership map and tell it
  to read the installed guide by its **absolute path**. Resolve the session
  docs directory from your prompt's References, then append `parallel-writes.md`.
  For example, session `s42` at the default Linux data root uses
  `/home/alex/.local/share/bus/sessions/s42/docs/parallel-writes.md`.
  Send the actual resolved path, not this example, a relative checkout path,
  `~`, or an unexpanded variable. Workers can open that file directly without
  access to your prompt.

Write the choice in the workflow's Coordination section.

Record which worktrees must remain until each checkpoint in the workflow.
Delegate a data-volume `df` check at kickoff and each phase boundary. If free
space falls below 30 GiB, declare a `DISK HOLD` and pause builds, benchmarks and
large copies. Delegate cleanup under the rules in `orchestrator-rules.md`, then
check space again before lifting the hold. Bus builds use a separate
`CARGO_TARGET_DIR` so the running Bus binary is not replaced.

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
  context has been compacted. On Bus's `reached N compactions` notice at
  **Settings → Max compactions per agent**, automatically follow
  [agent-handover](https://github.com/asura1234/bus/blob/master/workflows/agent-handover.md).
  Clear a worker before an unrelated task. The orchestrator never clears or
  replaces itself, because it carries the context of the whole effort.

## The review loop

Apply the [binding rule against review-loop whack-a-mole](orchestrator-rules.md#review-loop-convergence)
to every task, including a change that appears to have only one affected place.
Before implementation starts, have an investigator enumerate everywhere the
goal must hold: entry points, screens, callers, platforms, providers and states.
Each acceptance-checklist row names `file:line`, the scenario, and a reproduction,
test or other concrete way to check it. Put the same checklist in the worker's
brief and the PR. Keep it within the locked goal; a new requirement still needs
the developer's agreement. Bug-fix briefs still require evidence of the root
cause before behavior changes; enumeration does not replace that investigation.

For example:

- **Reference audio limit:** "refuse reference audio over 30 s everywhere" had
  about six entry points in another project. Each review or verification round
  found another one, costing a full fix → gate → review → verify loop; PRs stayed
  open for seven and twenty hours. Enumerate every picker, upload, reuse and
  provider route before coding, with a way to try an over-limit clip on each.
- **UI treatment:** list every affected screen and its loading, empty, error
  and populated states, including platform variants; check each treatment there.
- **Delivery fix:** list every provider and caller, including reconnect, retry
  and failure states; reproduce the delivery behavior for each. Evidence of the
  root cause must explain the bug before the fix is dispatched.

Run checks and remediation as a bounded loop:

1. Pin one head and give the checklist to two or more `review-pr` reviewers on
   different providers and to the verifier. Launch their work and any bot review
   (for example Copilot) concurrently. For delivery, callbacks or launch, the
   verifier also runs `just e2e` (live round trips with every provider) and reports
   its table in this same round, rather than waiting for review to finish.
   Keep the head unchanged while the checks run. Results for an older head do
   not prove a later fix.
2. Collect all findings before sending one consolidated fix brief to the author
   to run `address-review-comments`. A finding outside the checklist exposes an
   enumeration miss: have the investigator expand the list and recheck that
   entire category, then give the updated list to the author, reviewers and
   verifier. Do not fix only the reported instance.
3. For each open or flagged issue, poll the author and every reviewer for a
   recommendation. Forward all recommendations to the author to run the
   `best-of-n` skill. A clear winner (`universal` or `clear`) is applied by the
   author, and the round continues unblocked. With no clear winner, defer a
   nonblocking issue: record it in the workflow's deferred list, with its
   recommendations and why it does not block, and ask the developer later in
   MASTER at the next developer checkpoint. The round continues unblocked. For a
   blocking issue without a clear winner, halt progress and ask the developer in
   MASTER; resume only after their decision.
4. Count one fix round as one consolidated set of fixes followed by gates and
   concurrent review, verification and bot review of its new head. Allow at most
   **two fix rounds per PR** after the initial checks; record the count in the
   workflow. Do not start a third round automatically. If the core goal still
   fails or a required gate or blocking issue remains, stop and ask the developer
   in MASTER for a plan.
5. Once the core goal holds on the enumerated checklist and required gates pass,
   present the evidence, any deferred decisions and remaining edge work to the
   developer for their regression test and merge OK. The time-box never grants
   merge permission or drops a core requirement. Turn remaining edge work into
   follow-up PR steps in the running workflow, with an owner, dependencies and
   acceptance checks. Execute them in order unless a recorded pause or developer
   checkpoint comes first; a deferred note alone is not a follow-up step.

Track PR age from its creation, including time spent as a draft. Any PR open
longer than **four hours** is escalated in MASTER as `[waiting-on-you]`: link the
PR, state the cause, show the fix-round count, and propose a concrete plan and
the decision needed. Escalate even while checks are running or the two-round
limit has not been reached. Update the workflow after the developer's decision.

## Steer agents without waiting

Correct an agent the moment you notice a problem; do not wait for its turn to
end. `bus send` to a working agent types the message into its running turn, and
the turn's reply answers the original task and every correction together:
`bus wait` on any of those messages returns the same reply. Send with `--queue`
only for an unrelated task that should get a turn and a reply of its own.

A plain send never stops a turn: Codex takes it into the running turn and
Claude Code queues it, so "pause" or "stop" in a message is just more work. To
stop a worker, or to redirect it onto something else, interrupt it:

```sh
bus agent interrupt AGENT --as YOU --text "Stop. Reply with your status."
```

Bus presses the provider's stop key once, waits until the turn has ended and
reports `stopped`, `already_idle` or `failed` with a reason. `--text` then
arrives as a normal new message with its own reply. The interrupted message
closes as `interrupted`: a running `send --async` or `wait` on it ends with
`message_interrupted` instead of waiting on. Messages queued for the worker
close with it, so send what it should do next with `--text`. `--room ROOM`
stops every working or blocked agent in the room. Nothing is killed and the worker keeps
its context.

## Unblock agents

- **Dialogs:** agents stop at permission, trust and question dialogs and
  other screens they cannot pass alone. Bus sends you one notice about the
  blocked worker, "Blocked, needs help to continue.", the same for every blocker and
  once per episode; `bus wait` also stops early with
  `agent_waiting_on_dialog`. Look at its terminal: `bus agent dialog AGENT`
  shows the question or full command, the options and a fresh fingerprint
  (`bus agent read AGENT --source visible` for any other screen). Then `bus
  agent choose AGENT --option N --fingerprint F`, or for a free-text question
  `bus agent answer AGENT --text "..." --fingerprint F` (`--skip` instead of
  `--text`, except on Claude questions, where Esc cancels the form); `--text`
  also answers a choice's `free_text` option. Decide, answer, and check the reported `outcome`; nothing follows
  when the dialog closes.
  Ask the developer before approving anything destructive or outward-facing.
- **Stuck requests:** check `bus message status` (each request's `stage` and
  `reason`), the agent's `wait_reason` in `bus state`, and `bus agent read`
  before `bus request recover`.
- **Delivery is not completion.** Wait for `complete: true` from `bus wait` or
  `bus message status`.

## When to ask the developer

Apply [Never act on assumptions; ask the developer](orchestrator-rules.md#never-act-on-assumptions)
first. The following decision boundaries also apply.

Ask before anything outside the room's goal or the workflow's decision rules:

- Publishing, merging, pushing to shared branches, or deleting work. Adding
  and deleting agents in your own room is not on this list: do it as the
  workflow needs and log it.
- Changing the goal or dropping a requirement.
- A blocking review issue with no clear winner after the author runs
  `best-of-n`. Nonblocking issues without a clear winner stay in the workflow's
  deferred list for the next developer checkpoint and do not halt the round.
- A hard gate that no option can meet.

If the developer gave you full control for a decision, decide, record it in the
workflow log, and tell the developer in MASTER what you decided.
