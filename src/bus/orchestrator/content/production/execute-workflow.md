# Skill: execute-workflow

Use this skill to run the room's current SOP. After every room event you decide the next step yourself.

## Current SOP

Record the SOP source as a `ReadContent` reference name or a `ReadWorkflowDraft` draft id, and re-read it when you need it. If a read reports an unknown, missing, or mismatched source, treat that as a fact: inspect again, select another source, or ask the Human. Nothing falls back, copies, or recovers on your behalf.

## Each wake

1. Read `ROOM_FACTS` and any `TOOL_RESULTS`: new Human messages, settled Requests, settled operations, and changed work.
2. Compare them with the SOP and with the assignments you made.
3. Gather missing evidence with `InspectWork`, `WaitForChange`, `ReadAgent`, or `ReadWorkflowDraft`.
4. Decide the next step and make the calls: a new assignment, a follow-up question, a Human message, a permission decision, a resource lease, a recovery, an SOP revision, or nothing.
5. When nothing useful remains until facts change, stop.

The SOP diagram suggests order, loops, branches, and parallel lanes. The actual next step comes from current facts and your judgment. Never wait for the harness to advance the workflow.

## Interpreting settlement

- A background-pending callback is progress. The Request is still open.
- A trusted continuation in the same provider session rebinds to the same Request and waits for a later final reply.
- An ordinary final reply settles only that Request.
- Activity after settlement does not declare work complete. Evaluate replies and reported artifacts against the SOP's success evidence yourself.
- Recovery with `AbandonIdleRequest` exposes only `Abandoned` for that Request. You decide what happens to the work.
- A denied, stale, or uncertain result means you re-read the facts and decide again. It never triggers an automatic recovery.

## Assignments

- Delegate with `SendMessage` to a coding agent and give each piece of work a `work_id`. State the objective, the constraints from the Room Brief and the SOP, the evidence you expect, and who receives the report.
- Avoid simultaneous uncoordinated writers on the same files or resources. Use `AcquireResource` and `ReleaseResource` when partners share something scarce.
- Match model and effort to the difficulty of the work. Ask for independent recommendations when a decision matters.
- Send technical disagreements back to the responsible author with every recommendation attached.

## Permissions and recovery

- For a permission prompt, call `ObservePermissionPrompt`, then decide whether to call `ApprovePermissionOnce` with the observed fingerprint. Ask the Human about anything risky or ineligible.
- For a Request wedged on an Idle agent, confirm the exact facts and call `AbandonIdleRequest`. Then decide whether to reassign, try a different approach, or ask the Human.
- Replacing a participant is a Human action; ask by room message.

## Revising the SOP

Within the confirmed Room Brief, revise the SOP with `PersistWorkflowDraft` whenever reality diverges from it: add steps, loops, attempts, or lessons. Routine revisions need no approval. Publishing a standard workflow requires the Human's approval and then `PromoteWorkflowDraft`.

## Finishing

When the SOP's success evidence is present, send the Human a factual summary with the evidence and any remaining risks. Work that would exceed the Room Brief needs a new room or a Human-controlled action.
