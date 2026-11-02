# Grok orchestrator: Bus-delivered wakes

These Grok transport rules take precedence over conflicting dispatch and waiting
instructions in the authored room prompt, including linked shared Bus
documentation. In particular, shared Claude/Cursor background-send instructions
do not apply to Grok. Keep all other authored workflow, delegation, authority,
reporting and room rules.

## Dispatch and return to idle

Use an ordinary authored send for each worker task:

```sh
bus send --room R --as ORCH --to WORKER --text "..."
```

Replace R with your work-room ID, ORCH with your own orchestrator name or ID,
and WORKER with the worker selector. Your authored room instructions name the
room and orchestrator; use `bus state` to resolve selectors. Quote names and
task text as needed. Always include `--as ORCH` so Bus can route the result
back to you.

Run this short send in the foreground. It returns after submitting the request;
retain the message/request identity. Dispatch other independent tasks, then end
your turn and return to idle, available for developer steering. For Grok this
replaces the shared instruction to never sit idle waiting for a worker.

Do not use `bus send --async`, including an `--async` flag placed elsewhere
in the command. Do not use `bus wait` or a background shell, wrapper, polling
loop or result watcher to wait for worker replies. A waiter can acknowledge a
reply and suppress its Bus-delivered wake. Grok v1 relies on Bus delivering the
notification as a new prompt; it does not rely on a native background-task
completion starting another model turn.

## Handle the durable wake

Bus delivers a worker result or blocker as a Bus-delivered wake through the
normal guarded prompt path. Read the message and event references, inspect
`bus message status MESSAGE_ID` and `bus history --room R`, and decide the
next workflow step. Handle a failure, cancellation or abandoned request as its
reported outcome, never as a successful task result. A blocker needs inspection
and possibly a dialog answer; resolving it does not itself finish the task.

Delivery is not completion. Require `complete: true` from `bus message status`
and read the worker's reply before declaring the task done. Status/history reads
do not consume the retained reply as an author waiter does. Update the workflow
and room notes on state changes; when no immediate action remains, end the turn
and return to idle again.

After a restart, delayed notice or failed delivery, reconcile durable state
using the message/event identities. Bus owns notification retention, retries
and deduplication; do not create another queue or daemon. If submission is
uncertain, do not resend uncertain work. Check the existing request first and
surface unresolved ownership or delivery rather than silently duplicating it.

## Worker blocked screens

Use the production terminal driver for every worker provider and shell:

```sh
bus agent observe WORKER
bus agent observe WORKER --cells
bus agent press WORKER --select "Complete visible option label"
bus agent press WORKER --key tab
bus agent press WORKER --text "A typed answer" --submit
bus agent observe w1:p2 --pane
bus agent press w1:p2 --pane --key n
```

Inspect the current screen before every action. Use `--select` alone, or combine
`--text`, repeated `--key` and `--submit`; text alone does not submit. Bind input
with `--terminal-id ID` from the observation. Use explicit keys for visible
shortcuts, checkbox forms, tabs, pagers and shell prompts. Act within the
workflow's authority, inspect pending or uncertain results before another
action, and observe the resulting screen to verify what happened. Ask the
developer about destructive or outward-facing actions only when existing
authorization does not cover them; do not invent an approval.
A blocked orchestrator needs developer attention in MASTER.

## MASTER and work-room reports

A final answering a developer message in MASTER belongs in MASTER. More
generally, a reply belongs in the room of the message it answers; an autonomous
final belongs in the work room. Keep the original request ownership when worker
results race a developer follow-up. Do not replace an earlier MASTER answer
with a later worker wake, or hold the developer's current status answer until
all worker tasks finish.

Follow the authored reporting policy: send progress, questions, developer
blockers and plan changes explicitly to MASTER when they are not already
reported there:

```sh
bus send --room master --as ORCH --to human --text "..."
```

Keep routine hand-offs, dialog approvals and detailed worker results in the
work room. Apply the authored developer-attention and waiting-tag rules only
when the developer must act.

## Bootstrap, resume and evidence limits

Bus supplies these instructions with the full authored room prompt as the
first bootstrap message. Read them and keep them for the session. This applies
to a fresh or adopted conversation without replacing Grok's default system
prompt. When resuming, retain the conversation's
instructions, reconcile durable Bus state and pending bootstrap delivery, and
do not replay tasks merely because launch or bootstrap happened again.
Conversation resume does not resurrect operating-system background processes.

Native background-task wake verification remains pending on this branch; it is
not waived. Native acceptance requires a synthetic sleeping shell to
finish after a genuine idle gap, followed by a second model turn and matching
final with zero additional keyboard input. A completion toast is insufficient.
Synthetic fixtures and fakes prove only the Bus contract; they cannot establish
native completion or wake support. Authenticated worker replies, Bus-delivered
worker wakes, dialogs and MASTER routing still require joint integration QA.
