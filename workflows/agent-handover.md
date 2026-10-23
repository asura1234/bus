# Workflow: agent-handover

This workflow is a skill for the orchestrator: it shows how to replace a worker
without losing the details of its current task. Automatically follow it when Bus
sends `bus → orchestrator: AGENT reached N compactions; get a handover note and
replace it.` at **Settings → Max compactions per agent**. No new approval is
needed for this replacement. It applies only to workers: the orchestrator never
clears or replaces itself, and clearing a worker is not a substitute.

The orchestrator already has broad-stroke context about what's been done and
what's next, so those sections of the note are short, one-sentence bullets.
**What's ongoing now** carries the detail the orchestrator does not have.
Follow the steps and decisions below, using judgment for names and recovery.

## Graph

```mermaid
flowchart TD
    begin(["Bus: worker reached N compactions"]):::start --> note
    note["Old agent: write handover note from the template; stop task work at a safe checkpoint"] --> check
    check{"Orchestrator: note complete and repository state current?"}
    check -- no --> repair["Old agent: fill gaps or refresh the note"]
    repair --> check
    check -- yes --> add["Orchestrator: add a fresh agent with the same provider, role and PWD"]
    add --> brief["Orchestrator: brief fresh agent from the note; ask it to raise anything unclear before continuing"]
    brief --> questions{"Fresh agent: anything unclear?"}
    questions -- yes --> answer{"Orchestrator: can you answer?"}
    answer -- yes --> explain["Orchestrator: answer from the note and room context"]
    explain --> questions
    answer -- no --> ask["Orchestrator: forward unanswered questions to developer in MASTER; pause handover and dependent work"]:::developer
    ask --> reply["Developer: answer in MASTER"]:::developer
    reply --> explain
    questions -- no --> acknowledge["Fresh agent: acknowledge the handover; wait for ownership transfer"]
    acknowledge --> delete["Orchestrator: delete old agent"]
    delete --> log["Orchestrator: log replacement; update participants, room notes, roster and affected briefs"]
    log --> announce["Orchestrator: announce replacement name/ID to PR peers and shared-file owners"]
    announce --> resume["Orchestrator: tell fresh agent to continue"]
    resume --> done(["Worker replaced"]):::success

    classDef start fill:#dbeafe,stroke:#1d4ed8
    classDef success fill:#dcfce7,stroke:#15803d
    classDef developer fill:#fde2e4,stroke:#c9184a
```

Ask the old agent to copy [handover-template.md](handover-template.md) to
`temp/handover/<agent>-<date>.md` in its PWD, using its agent name and a
`YYYY-MM-DD` date. Use a distinct suffix if that path already holds a different
handover. Record the absolute note path so the orchestrator and fresh agent can
both read it. The old agent finishes or safely stops in-flight work, records
anything still running, and makes no further task changes after writing the
note unless asked to refresh it.

Read the whole note before adding the replacement. Check that it covers the
exact current step, running or half-done work, touched files, commands in flight,
open questions, and gotchas. Every worktree and branch involved in the task
must have its current full commit hash recorded, including branches not checked
out. Every staged, unstaged, and untracked change must be listed and classified
as either **part of the task: commit-and-push when done** or **exploratory: safe
to stash**. Have the old agent verify the snapshot against Git and resolve gaps;
do not assume an omitted worktree is clean or automatically stash anything.

Use `bus agent add --room ROOM --name NEW --provider PROVIDER --pwd PWD`
with the old agent's provider and exact PWD, preserving its task role in the
brief. Include the worker brief messaging rule from the handover template.
Start a fresh session; do not resume the old context. Brief it with the
note's absolute path, the task goal and boundaries, and this instruction:
"Read the handover note and ask questions about anything unclear before
continuing. Wait for ownership transfer before changing files or running task
commands."

The orchestrator is expected to answer most questions itself from its
broad-stroke context, the note, and the room history. Only questions it cannot
answer go to the developer in MASTER. Pause this handover and any work that
depends on those answers until the developer replies; relay the answers to the
fresh agent before continuing. Keep the old agent available through this check.

After the fresh agent understands the handover, run
`bus agent delete OLD --confirm`. Deleting the agent does not authorize deleting
worktrees, branches, files, or uncommitted changes. Log the date, compaction
notice, old → new agent names, provider, role, PWD, and absolute note path in the
room's running workflow log. Update its participants, room notes, PR peer roster
and affected briefs. **Announce the replacement's name/ID to every PR peer and
shared-file owner before telling the fresh agent to continue** from the recorded
current step. The orchestrator owns these updates; the worker reports peer gaps
back to it.

## Failure recovery

- **Unanswered question:** the developer answers in MASTER; the orchestrator
  relays the answer and resumes the paused handover at the question check.

## Orchestrator failure recovery

You handle these yourself:

- **Incomplete or stale note:** ask the old agent to fill the missing details
  or refresh the Git snapshot, then read and check it again before proceeding.
- **Agent add or briefing failed:** keep the old agent and all repository state
  intact, resolve the failure, and retry from the failed step; do not start two
  workers writing the same task.
- **Old agent unavailable:** recover the note and repository state through a
  worker without granting it task ownership yet; if essential ongoing details
  cannot be recovered, ask the developer in MASTER and pause dependent work.
- **Delete failed:** keep the fresh agent waiting for ownership transfer and
  retry deletion before logging a completed replacement or resuming task work.

## Participants

| Role | Model | Does |
|---|---|---|
| Orchestrator | any | Automatically starts this workflow on the worker's notice, checks the note, adds and briefs the fresh agent, answers most questions, escalates only unanswered ones to MASTER and pauses for answers, deletes the old agent, logs the replacement, updates the roster/briefs and announces the new name/ID to peers before resuming the fresh agent; never replaces itself. |
| Old agent | existing provider | Writes the detailed note and complete repository snapshot, stops task changes at the handover checkpoint, and remains available until the fresh agent understands the task. |
| Fresh agent | same provider as old agent | Starts in the same PWD and role, reads the note, asks questions before continuing, and waits for ownership transfer. |
| Developer | - | Answers only questions the orchestrator cannot answer, in MASTER. |

## Worker brief messaging rule

Bus automatically adds `name (agent id, room R):` to agent-authored messages. Send only the body; do not add a "from X", "X here" or signature line.
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
