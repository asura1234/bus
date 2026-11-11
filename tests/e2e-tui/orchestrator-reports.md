# Orchestrator reports — where an orchestrator's turn result shows

Apply app.md's [run contract and setup recipes](app.md#run-contract-and-setup-recipes)
(R, A, P and the fixture rule). Source: `src/messaging/coordinator/reports.rs`.

An orchestrator turn started by a message settles that request as `ended`
without storing or posting the native final as a reply. Its authored message
remains in the original conversation: MASTER for a developer's MASTER message,
the work room for a worker's message or Bus's Blocked notice. A turn no message started
(a background command exiting, or text typed into the orchestrator's own
terminal) is routine status: Bus posts it in the work room the orchestrator runs,
as `ORCH → You`, never in MASTER. Progress, questions and blockers reach MASTER
only when the orchestrator posts them with `bus send --room master --as ORCH
--to human` (the orchestrator prompt and rules say when).

## Cases (run order)

### ORCH-01 — Native completion stays out of replies; autonomous status and explicit posts use their rooms

**Preconditions/setup:** R(`qa-work`); A(`qa-work`, `qa-worker`, Claude Code);
then the orchestrator `qa-orch` with APP-04's UI recipe (Claude Code, scratch PWD,
`Orchestrates room` = `qa-work`). Wait until both fake terminals are ready and
`bus state` shows both agents idle.

Step 1 opens an inert approval fixture on the worker's screen using the fake
agent's `fake:dialog-settle redraw` command. This renders native menu controls and
waits for input; the displayed shell command is never executed. The generic driver
reports the controls as blocked. The fake orchestrator completes Bus's Blocked
notice without a stored reply or an answer to the approval. Step 8's progress
report is an agent-authored post through the scratch CLI, as the fixture rule
allows: it is what `send --to human` does when an orchestrator follows its prompt.

1. Click `# qa-work`, click the `qa-worker` agent (its fake terminal opens) and
   type this fixture command and press Enter:
   ```
   fake:dialog-settle redraw
   ```
   Verify the worker shows `Would you like to run the following command?` and
   numbered Yes/No options. Wait until `bus state` shows `qa-worker` blocked, then
   press F6 → `qa-worker` shows `Blocked` in the sidebar, and the qa-work history
   shows Bus's notice to `qa-orch`: `qa-worker. Blocked, needs help to continue.`
2. Wait until `bus state` shows `qa-orch` idle → its Blocked-notice request is
   complete with `stage:ended`, `reply:null` and no failure reason. Its native
   pane shows `fake reply: qa-worker. Blocked, needs help to continue. …`.
   In qa-work the original Blocked message has no reply card, and no `qa-orch → You`
   message was synthesized from that accepted final.
3. Click `# MASTER` → MASTER history shows no `Blocked, needs help to continue.`
   text and no message from `qa-orch`.
4. Click the `qa-orch` agent → its fake terminal opens. Type `ORCH01 background done`
   and press Enter; wait for `fake reply: ORCH01 background done` on its screen and
   for `bus state` to show `qa-orch` idle. Press F6 → MASTER chat shows no
   `ORCH01 background done` text and no message from `qa-orch`.
5. Click `# qa-work` → its history ends with a `qa-orch → You` message
   `fake reply: ORCH01 background done`.
6. Click `# MASTER`, P(`qa-orch`), type `ORCH01 status please` and press Enter →
   once the delivery settles, its request is complete with `stage:ended`,
   `reply:null` and no failure reason. MASTER history contains the developer's
   `ORCH01 status please` exactly once with no reply card; the exact fake output
   is visible only in `qa-orch`'s native pane.
7. Click `# qa-work` → its history does not contain `ORCH01 status please`, and
   step 5's message is still its only `qa-orch → You` message.
8. Fixture: `bus send --room master --as qa-orch --to human --text "ORCH01 progress: PR merged"`
   → the send returns `"stage": "posted"` with no request IDs. Click `# MASTER` →
   its history ends with a `qa-orch → You` message `ORCH01 progress: PR merged`.
9. Click `# qa-work` → its history does not contain `ORCH01 progress: PR merged`.

**Evidence:** screen text (`find`, `snapshot --plain`) of each room after each step,
a cells screenshot of each room's history, `bus state` agent status, and
`bus history --room master` and message status for step 6, and the step-8 send receipt.

**Pass criteria:** message-owned native finals settle as ended without replies
or synthesized posts; original messages remain in their conversations. The
unprompted status shows only in the work room, and the explicit `send --to human`
progress report shows only in MASTER.
