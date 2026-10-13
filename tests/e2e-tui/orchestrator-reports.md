# Orchestrator reports — where an orchestrator's turn result shows

Apply app.md's [run contract and setup recipes](app.md#run-contract-and-setup-recipes)
(R, A, P and the fixture rule). Source: `src/messaging/coordinator/reports.rs`.

An orchestrator's turn final shows in exactly one room. A turn that answers a
message ends in that message's reply, under it in the message's room: MASTER for
the developer's message, the work room for a worker's (such as Bus's "Blocked,
needs help to continue." for a worker's open dialog). A turn no message started
(a background command exiting, or text typed into the orchestrator's own
terminal) is routine status: Bus posts it in the work room the orchestrator runs,
as `ORCH → You`, never in MASTER. Progress, questions and blockers reach MASTER
only when the orchestrator posts them with `bus send --room master --as ORCH
--to human` (the orchestrator prompt and rules say when).

## Cases (run order)

### ORCH-01 — Routine status stays in the work room; progress reports and MASTER replies show in MASTER

**Preconditions/setup:** R(`qa-work`); A(`qa-work`, `qa-worker`, Claude Code);
then the orchestrator `qa-orch` with APP-04's UI recipe (Claude Code, scratch PWD,
`Orchestrates room` = `qa-work`). Wait until both fake terminals are ready and
`bus state` shows both agents idle.

Step 1 stages a permission dialog on the worker's screen. It pastes a Claude Bash
permission prompt into the worker's own terminal and does not press Enter, so the
text stays in the fake Claude's prompt box, and Bus's dialog detection reads it as
a blocked worker (`bash_permission_prompt` in
`src/agents/manifest/bundled/claude.toml`). Do not send the text as a room message:
Bus pastes it and presses Enter, and whether its dialog poll sees the text before
the Enter is a race. The orchestrator's reply to that Blocked message stands in for a
routine dialog approval. Step 8's progress report is an agent-authored post through
the scratch CLI, as the fixture rule allows: it is what `send --to human` does
when an orchestrator follows its prompt.

1. Click `# qa-work`, click the `qa-worker` agent (its fake terminal opens) and
   paste this text into it, without pressing Enter:
   ```
   ORCH01 stage Bash(echo hi)
   Do you want to proceed?
   ❯ 1. Yes
     2. No
   Esc to cancel
   ```
   Wait until `bus state` shows `qa-worker` blocked, then press F6 → `qa-worker`
   shows `Blocked` in the sidebar, and the qa-work history shows a
   `qa-worker → qa-orch` message `qa-worker (agent N). Blocked, needs help to continue.`
2. Wait until `bus state` shows `qa-orch` idle → in qa-work, `qa-orch`'s reply
   `fake reply: qa-worker (agent N). Blocked, needs help to continue. …` shows
   under the Blocked message, and qa-work has no `qa-orch → You` message.
3. Click `# MASTER` → MASTER history shows no `Blocked, needs help to continue.`
   text and no message from `qa-orch`.
4. Click the `qa-orch` agent → its fake terminal opens. Type `ORCH01 background done`
   and press Enter; wait for `fake reply: ORCH01 background done` on its screen and
   for `bus state` to show `qa-orch` idle. Press F6 → MASTER chat shows no
   `ORCH01 background done` text and no message from `qa-orch`.
5. Click `# qa-work` → its history ends with a `qa-orch → You` message
   `fake reply: ORCH01 background done`.
6. Click `# MASTER`, P(`qa-orch`), type `ORCH01 status please` and press Enter →
   once the delivery settles, MASTER history shows the developer's
   `ORCH01 status please` and `qa-orch`'s reply `fake reply: ORCH01 status please`
   under it.
7. Click `# qa-work` → its history does not contain `ORCH01 status please`, and
   step 5's message is still its only `qa-orch → You` message.
8. Fixture: `bus send --room master --as qa-orch --to human --text "ORCH01 progress: PR merged"`
   → the send returns `"stage": "posted"` with no request IDs. Click `# MASTER` →
   its history ends with a `qa-orch → You` message `ORCH01 progress: PR merged`.
9. Click `# qa-work` → its history does not contain `ORCH01 progress: PR merged`.

**Evidence:** screen text (`find`, `snapshot --plain`) of each room after each step,
a cells screenshot of each room's history, `bus state` agent status, and
`bus history --room master` for the step-6 reply, and the step-8 send receipt.

**Pass criteria:** the worker-woken reply (a routine approval) and the unprompted
status show only in the work room; the reply to the developer's MASTER message and
the orchestrator's `send --to human` progress report show only in MASTER.
