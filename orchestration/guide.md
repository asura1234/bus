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

Bus launches every agent with messaging instructions. It launches you with an
orchestrator system prompt (editable in the TUI form,
or `--system-prompt`/`--system-prompt-file`) that names your room and points to
the Bus docs in `<BUS_DATA_DIR>/docs/`. Bus writes nothing into your PWD. A new
PWD may show the provider's "trust this folder" prompt on first launch; the
developer answers it in your terminal. Your room is fixed for your whole life:
there is no reassignment, and deleting the room deletes you with it.

On your first turn:

1. Run `bus state` and find your room, its agents, and your own agent ID.
2. Read the room's notes and history: `bus history --room "$room_id"`.
3. Read the agreed workflow and its authorizations (next section); resolve only
   missing goal or authority decisions before the affected work starts.

After a restart, reconcile `bus state` and GitHub before any other work. Match
each owned PR or dependency to its current head, reviews, checks, acceptance and
merge state; read pending replies with `bus message status`. Reconstruct the
next owners, live waits, dependencies and explicit holds in the workflow and
room notes. A dead waiter or deleted worker is not a live wait. Treat an unrelated
GitHub merge as informational until it matches this room's ownership or dependency.

Send every message to the room's agents as yourself, so the history shows who
asked:

```sh
bus send --room "$room_id" --as claude-orch --to claude-dev --text "..."
```

Bus adds one sender header: `name:` locally, `name (room R):` across rooms or MASTER; IDs appear only for numeric or historical names. Send only the body; do not add a "from X", "X here" or signature line.
Include this rule in worker briefs. Any agent can message agents in any room;
`--to orchestrator` names the sender's home room's orchestrator.

### Move an existing session into MASTER

Use `bus agent add --help` for supported orchestrator providers, session adoption
flags, original working-directory requirements and prompt delivery.

## The workflow file

Each room has a workflow file, a living plan the orchestrator keeps: goal,
non-goals, a mermaid graph of the steps, participants, gates, coordination
rules, decision rules, and a log. Follow
[workflow-create](workflow-create.md) (in the Bus docs folder) and start from
[the workflow template](templates/workflow-template.md). For workflow lookup,
the resolved directory and shipped names, follow the
[binding named-workflows rule](orchestrator-rules.md#named-workflows).

- **Draft from the agreed goal and authorizations.** Reuse existing decisions;
  take goal changes and unresolved product decisions to the developer. The graph
  is a reference, not a script, and its recorded checkpoints still apply.
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
5. `PR peers:` the live `PR → owner → verifier → reviewer` roster, using agent
   names and IDs. Update it and affected briefs whenever an agent is replaced.

```sh
bus room notes "$room_id" --text "Workflow: ~/bus-data/workflows/pr-123.md
WAITING ON YOU:
- Approve claude-dev's push of feat/gallery (bus agent dialog claude-dev)
- Pick a gallery list: native list or RN lib (both pass benchmarks)
Now: review round 2 (claude-review, codex-review)
PR peers: #123 → claude-dev (11) → codex-verify (12) → claude-review (13), codex-review (14)
DONE / OPEN:
[x] Spec agreed
[x] Implement gallery (claude-dev)
[x] CI green
[ ] Review, verification and bots (continue until every reviewer is Ready)
[ ] Manual test (if the workflow holds for it)
[ ] Merge (auto-merge-pr / recorded hold)"
```

`room notes` replaces the whole field, so always send the full board. Update it
when the state changes, not on every message.

## Open peer messaging

Make direct peer messaging the normal path for each PR's implementation,
verification, review and merge loop. The orchestrator staffs the team and keeps
the room's plan current; it does not relay each head, verdict or shared-file
request. Routine progress can continue while the orchestrator is unavailable.
The PR owner is its author; these are the same role throughout this SOP.

Before work starts, put the PR's **owner, verifier and reviewer** names and IDs
in every worker brief, with the PR (or task key until it exists), worktree,
branch, acceptance checklist and current head. List every required reviewer.
Keep the same `PR → owner → verifier → reviewer` roster in the room notes.
Assign **one verifier per 1–2 PRs**; distribute more PRs across additional
verifiers instead of making one verifier the room's hub.

1. **Owner → peers:** send the PR, exact full head SHA, worktree and checks to
   the assigned verifier and reviewers. On **every push**, re-send the new head
   and the delta to those peers; a verdict for an old head does not prove it.
2. **Verifier → owner:** send `VERIFIED <PR/task> @<full SHA>` with the acceptance
   evidence, or defects with `file:line`, reproduction and the affected head,
   straight to the owner. Re-verify each new head. Reviewers likewise send their
   verdict and findings directly to the owner in their assigned review lanes.
3. **Owner → merge:** use the owner-run variant of
   [auto-merge-pr](workflows/auto-merge-pr.md). Before every repair, exclude all
   findings outside the locked goal and report them to the orchestrator in the
   final as follow-ups. Any FLAG or developer decision stops this PR's merge
   path and goes to the orchestrator for triage. Keep fixing until every
   reviewer is Ready. Hold and escalate sharp reviewer disagreements, goal
   changes, and destructive or outward-facing actions without existing
   authorization. Re-measure added plus removed lines against the PR base,
   excluding `plans/`, after pushes and before merge; at **3000 lines**, stop
   and hand splitting back to the orchestrator. Merge only with exact-head
   `VERIFIED`, required reviews Ready, gates and requested acceptance passing,
   comments adjudicated, no unresolved FLAG/decision, and no explicit hold.
   Reuse recorded authority; peer messaging grants no new authority or waiver.
4. **Shared-file owners → each other:** message before editing, even in separate
   worktrees. Agree who applies which edits and in what order, using the
   ownership map and grant/release protocol in [Parallel writes](parallel-writes.md).
   If ownership must change or a conflict cannot be resolved within the map,
   bring that part to the orchestrator while independent work continues.
5. **Finals → orchestrator:** the PR owner sends milestone finals and blockers
   to the orchestrator, once each, with the PR/head, peer verdicts, evidence and
   next owner or hold. Reviewers and verifiers send their verdicts to the owner
   only (see [Bus communication guide](bus-communication-guide.md)). Start milestone
   finals with `READY FOR VERIFY`, `REVIEW READY`, `MERGED`, `DONE` or `BLOCKED`.
   A peer-started turn replies to that peer, so also send the milestone report
   to `--to orchestrator`; a reply to an orchestrator-started task already goes
   there. Send only the body; Bus adds the header, so never sign messages.

The orchestrator retains **staffing and recovery, dialogs, product decisions,
and announcing replacements**, including room notes, the roster and verifier
allocation. Workers use the peers in their brief and message the orchestrator
when a peer is missing or replaced; they do not edit room notes or the roster,
or assign verifiers. The orchestrator resolves staffing gaps, unavailable agents
and escalations, handles dialogs under existing authority, and takes unresolved
product decisions to the developer. Whenever an agent is replaced, it updates
the room-notes roster and affected briefs and announces the new name/ID to all
of that agent's PR peers and shared-file owners before work resumes. Peers then
message the replacement directly. Do not route routine technical exchanges
through the orchestrator or one room-wide verifier.

For a peer-handled `READY FOR VERIFY`, the brief already names the verifier.
When the orchestrator records that roster assignment, it records
`verifier <name> engaged by owner` with the PR/head. It does not dispatch the
verification a second time. If no verifier is assigned, staff one or record a hold.

## Process every result

Preserve the full stdout/stderr and result body from `bus send --async` and
`bus wait`, including message IDs, terminal status and replies. Keep the original
output in the tool result or a durable artifact even if you also parse it. Never
replace it with `sent`, redirect it to `/dev/null`, or retain only lines matching
a retry grep. A retry filter can hide a successful Ready or merge result.

When a completion notification arrives, read the actual reply with
`bus message status`; the command description describes the assignment, not its
outcome. Immediately turn the result into an executing next step or an explicit
disposition: Ready gets a verifier or a hold reason, and merged/done gets cleanup
or reuse. Do independent cleanup before a new blocking verifier wait, or dispatch
cleanup and verification separately. A cleanup loop after a long `--async` wait
has not executed merely because you announced it.

Consume coalesced result notifications item by item, matching each event to its
message, PR/head and owner. Record the next action or reasoned hold before treating
it as acknowledged. Transport delivery or `seen` is not evidence of action.
Keep this ledger durable across restarts; deduplicate repeated events and re-arm
only on new facts. Do not add blind polling loops or wake for every chat line.

Before ending a turn, give **every active item** one of: a next executing owner,
a live durable wait, a named dependency, or an explicit developer hold. Record
the message/wait ID, dependency or hold reason, rather than just "waiting".
For example:

| Item / head | Last result | Next owner or wait/dependency/hold | Next action |
|---|---|---|---|
| PR / exact head | Ready for verification | verifier / dispatched message ID | Verify acceptance checklist |
| PR / merged head | Merged | cleanup worker / dispatched message ID | Close worktree and release resources |
| Device task / build | Prepared | named device dependency / explicit hold | Resume when access is available |

Never delete a worker until every required follow-up it carried has another
executing owner or a secured continuation of the kinds above. Update both room
notes and the workflow log, and report milestones in MASTER.

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
Use the resource protocol below for disk, builds and devices. A resource hold
blocks the operation that needs that resource; keep light reviews, investigation,
notes and other independent work moving.

## Resource leases and build slots

Check data-volume free space at kickoff and every phase boundary. Below 30 GiB,
hold builds, benchmarks and large copies, clean only owned regenerable output,
and recheck before resuming those operations. Orchestrators coordinate shared
disk/device status and scoped cleanup directly with each other; routine capacity
and scheduling decisions do not need a new developer approval.

Use expiring build/device leases. Record the owner, operation, process or worker
identity, resource, checkout/device, expiry and next liveness check in the workflow.
Renew only while the owner and operation remain live. Expiry triggers a liveness
check; it does not authorize stealing an active build or device. Release a stale
lease only after proving its operation has ended. If ownership is unclear,
coordinate with the other orchestrator while unrelated work continues.

Keep **one Cargo target per worktree**. For every Cargo, gate, coverage and E2E
run, export the absolute worktree target; configure coverage to reuse it too:

```sh
export CARGO_TARGET_DIR="$worktree_path/target"
export CARGO_LLVM_COV_TARGET_DIR="$CARGO_TARGET_DIR"
```

Do not create per-run targets such as `target/scratch`, `target/e2e`,
`target/submit-budget` or `/private/tmp/*-target`. Serialize conflicting builds
inside one worktree. Separate worktrees each reuse their own target, never a
shared writable target or the live Bus checkout's target. After each gate/E2E,
remove inactive per-run targets you created, retain only the worktree target,
and report deleted paths and measured sizes. Close a merged PR's worktree with
`worktree-close`, preserving source/evidence and checking for unpushed work first.

Each room permits **at most two full builds**. The orchestrator takes its room
id from its prompt and resolves `/tmp/bus-<room id>-build-slot-1` and
`/tmp/bus-<room id>-build-slot-2`. Record both literal absolute paths in the
workflow and pass them in every build worker's brief. Workers use those supplied
paths; they do not infer a room id. If the paths are missing, ask the orchestrator.

Before a full gate, coverage run or E2E binary build, set `build_slot_1` and
`build_slot_2` to the literal paths from the brief, then atomically acquire one
slot, recording which `mkdir` succeeded; if both exist, wait 60 seconds and retry:

```sh
mkdir "$build_slot_1" 2>/dev/null || mkdir "$build_slot_2" 2>/dev/null
```

Write owner/liveness/expiry metadata for the acquired slot before building.
Install an exit trap that removes only your metadata and slot directory on
success or failure. On interruption, stop and wait for the build's children
before releasing the slot. Do not delete another live owner's slot. Keep
non-build work moving while slots are occupied.

## Preflight long or overnight device work

Before dispatch, verify that target devices are connected, unlocked and trusted,
UI Automation is enabled, and the intended app, account/environment and a
representative fixture are available. Record any unavailable physical access as
a dependency on that device operation, not a hold on the whole room.

Ask the developer once for a bounded app relaunch/retry lease, unless one already
covers this work. Record the app/device, allowed relaunch/retry count, permitted
exit/recovery condition and expiry. Preserve app data and login. Use the lease
within those bounds; stop the affected operation on expiry, developer intervention,
an explicit pause or changed scope. It does not grant passcode access, permission
to erase data, or acceptance waivers. A build/install is not requested device
acceptance; the actual acceptance still has to pass on the exact head.

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

Run checks and remediation until every reviewer is Ready:

1. Pin one head and give the checklist to two or more `review-pr` reviewers on
   different providers and to the verifier. Launch their work and any bot review
   (for example Copilot) concurrently. For delivery, callbacks or launch, the
   verifier also runs `just e2e` (live round trips with every provider) and reports
   its table in this same round, rather than waiting for review to finish.
   Keep the head unchanged while the checks run. Results for an older head do
   not prove a later fix.
2. Have peers send findings directly to the owner, who collects them before
   running one consolidated `address-review-comments` batch. A finding outside
   the checklist exposes an enumeration miss: have the investigator expand the
   list and recheck that entire category, then give the updated list to the author, reviewers and
   verifier. Do not fix only the reported instance.
3. For an in-goal technical issue without a FLAG, have the owner collect
   recommendations directly from every reviewer, add its own and run the
   `best-of-n` skill and apply its top-ranked recommendation. Sharp reviewer
   disagreements go to the orchestrator for a decision. If an issue reaches the
   developer decision boundary below, or any item is FLAGGED, the owner stops
   this PR and sends a final to the orchestrator for triage. The orchestrator
   asks in MASTER when a developer decision is needed; independent work continues.
   Owners exclude out-of-goal findings and report them in finals for the
   orchestrator to decide and record. Turn remaining nonblocking work into follow-up
   steps with its recommendations and acceptance checks.
4. Keep fixing until every reviewer is Ready. Follow each consolidated repair
   batch with gates and concurrent review, verification and bot review of its
   new head. Hold and escalate only real decisions: sharp reviewer disagreements,
   goal changes, or destructive or outward-facing actions without existing
   authorization. Keep independent work moving while a decision is pending.
5. Once the core goal holds on the enumerated checklist and required gates pass,
   report the evidence and route the PR, once every reviewer is Ready, through
   [auto-merge-pr](workflows/auto-merge-pr.md). Reuse its recorded start answer;
   honor an explicit manual-test or approval hold without adding a new per-PR
   merge-OK gate. Turn remaining edge work into follow-up PR steps in the running
   workflow, with an owner, dependencies and
   acceptance checks. Execute them in order unless a recorded pause or developer
   checkpoint comes first; a deferred note alone is not a follow-up step.

When a real decision blocks a PR, link the PR, state the cause, and propose a
concrete plan and the decision needed. Escalate to the developer in MASTER only
when the orchestrator cannot resolve it within existing authority. Update the
workflow after the decision.

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
  Ask the developer about destructive or outward-facing actions only when
  existing workflow or developer authorization does not cover them.
- **Stuck requests:** check `bus message status` (each request's `stage` and
  `reason`), the agent's `wait_reason` in `bus state`, and `bus agent read`
  before `bus request recover`.
- **Delivery is not completion.** Wait for `complete: true` from `bus wait` or
  `bus message status`.

## Keep working; report status, don't ask permission

Follow the [binding keep-working rule](orchestrator-rules.md#keep-working).
The developer needs status: what is running, its state, and what comes next.
Routine permission questions transfer the orchestrator's job back to the
developer and leave workers idle while a reply is unavailable.

Advance in-scope work yourself: assign a newly found bug for evidence-based
root-cause analysis and a fix, start the next workflow step, dispatch review
fixes, and execute follow-ups. Log your decision in both room notes and the
workflow log, report status in MASTER, and keep workers busy. A new bug inside
the agreed goal does not need a new permission question; an unrelated bug does
not silently widen the goal.

Check existing instructions, kickoff approval and workflow decisions before
asking. Routine scheduling, reversible engineering defaults, mandatory
verification and the next approved step belong to the orchestrator. Resolve a
rebase conflict that preserves both sides' intent, fix a test, make a small
in-scope refactor or choose the recommended technical option, then state the
decision, its evidence and the next executing owner. Do not request that
authority again. A force-push to a shared branch others build on, deletion of
someone else's work, or a goal change still needs the developer's decision.

The anti-pattern is **"found another bug, should I fix it?" at 1 am, followed
by the room sitting idle for eight hours**. Investigate and fix an in-scope bug,
record the decision and report its status; keep other work moving during the
investigation.

Ask the developer only for:

- Unknown developer intent: an unknown workflow or skill name, unclear scope,
  a missing referenced resource or conflicting instructions. Stop the affected
  work and ask under the assumptions rule.
- A destructive or outward-facing action outside existing workflow or developer
  authorization, such as publishing or deleting work not covered by that authority.
- A product decision you have no basis to recommend on. Technical uncertainty
  calls for investigation and author-led adjudication, not a routine permission
  question.
- A change to the goal, including dropping a requirement.

[Never act on assumptions](orchestrator-rules.md#never-act-on-assumptions) still
applies to unknown intent: stop and ask rather than inventing a resource, scope
or authority. It does not require re-asking for
authority already given. Missing intent or conflicting resources block only the
affected part. When a developer decision is needed, ask in MASTER
with `[waiting-on-you]`, the cause and a recommended plan where there is a basis
for one. Hold only the affected part and keep every independent piece moving.

Approval of the workflow at kickoff covers its whole path through review and
[auto-merge-pr](workflows/auto-merge-pr.md), read in full. Merge only the exact
head whose reviews are Ready, gates and requested acceptance pass, and comments
are adjudicated, with no explicit hold. Reuse kickoff approval as the recorded
start answer instead of adding per-PR merge OK. New commits need current-head
evidence; authority does not waive an invariant or requested acceptance.
Manual-test/approval holds still apply. An explicit instruction to leave a PR
as a draft or not merge it remains a hold.

## Drive throughput

Follow the [binding throughput rule](orchestrator-rules.md#drive-throughput).
Run this audit every few events and on a timer while work remains open:

- Read PR state: what blocks each Ready PR, and which can advance now?
- Check resource and agent state yourself: what sits idle, and is each reported
  blocker real? Use a cheap read-only check before pausing or escalating.
- Find duplicate checks, stale holds and notes that widened a locked goal.
  Change bottleneck steps within [your authority](orchestrator-rules.md#authority-check),
  log the decision and next owner, and keep acceptance inside the goal.
- When finals go quiet or Ready PRs appear without a message, read `bus state`,
  `bus history` and `bus message status`; recover delivery, chase or re-staff.
  Apply [process every result](orchestrator-rules.md#process-every-result).
- After a rebase, compare `git range-diff` or before/after PR diffs. Record an
  unchanged own diff to carry verification forward; re-run fast gates. Use the
  slow resource again only when the PR's own changes moved.

For example, you have seven open PRs; Ready work stands still for about 1h45m,
and only two merge in about 3h. You trust an unreachable-device report and pause
the queue, copy old developer-only/designer holds into briefs, require verifiers
to repeat the owner's exact-head checks, and add an out-of-goal check that blocks
VERIFIED. You acknowledge and relay messages while two merges and four Ready
PRs go unseen for about 45 minutes during a delivery outage.
Instead, check the device list yourself: it shows availability in two seconds.
Reconcile PR and delivery state, drop stale holds, remove the added scope, reuse
the exact-head evidence and carry it through unchanged-diff rebases. Log the
process change and assign the next work to idle capacity immediately.

## Evidence behind the result and continuation rules

The **room 8259 (`libtv-app master`) 36-hour record** supplied for this SOP
contains **366 agent-to-agent messages**. Verification, review and merge loops
ran directly between peers, covering **10 merges overnight**. Shared-file owners
coordinated with each other, and the room kept merging during the orchestrator's
**5-hour usage limit**. This supports making peer loops standard while retaining
staffing, recovery and product decisions with the orchestrator.

The working briefs were `temp/bugs/briefs/bus-messaging.md` and `verify.md` in
the main `libtv-app` checkout. They named PR groups, required verifier defects
with `file:line` to go directly to the owner, required re-verification of each
pushed head, and kept final reports with the orchestrator. Their trial-era
restriction on addressing MASTER is superseded by current Bus messaging:
`--to orchestrator` reaches the sender's home room's bound orchestrator.

The **2026-10-22 room 877 disk incident** left only **893 MB free**, with roughly
**81 GB** in per-run Cargo targets. The immediate coordination used
`/tmp/bus-877-build-slot-1` and `/tmp/bus-877-build-slot-2`. This motivates one
target per worktree, inactive-output cleanup and bounded builds; the general
protocol above resolves a separate pair of slot paths from each room's id.

The **2026-10-22 wake-up RCA**, “Which fixes are actually needed”, item 4
(`temp/orchestrator-wake-rca/README.md` in the main Bus checkout), records
[#1602 — 项目：复制后保留画布节点顺序](https://github.com/liblib-ai/libtv-app/pull/1602):
Ready for verification at **17:38**, its result swallowed into `sent` twice
(the repeated reply was at 18:21), and a verifier request only at **19:01**.
Times are Asia/Shanghai. Delivery to a shell did not produce an executing next
step; keeping and reading the reply is essential.

The companion **2026-10-22 idle audit**, “What to change in the current rules”
and recommendations 1–5 (`temp/orchestrator-idle-audit/README.md`), records a
required follow-up queued and then left without an owner when its worker was
deleted. It also records a disk hold stopping light work and build-slot capacity
being escalated instead of coordinated. Those cases motivate secured next
owners, operation-scoped holds, expiring leases and direct resource coordination.

The audit's device cases include a build ready before device access was arranged
and an app-relaunch question left waiting overnight. Preflight and a bounded
retry lease address those dependencies in advance. Earlier explicit phase/device
holds were valid under their original authority; these rules do not retroactively
waive holds or turn every observed waiting hour into preventable idle time.
