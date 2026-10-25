# Idle agents alert

Settings includes **Idle agents alert (1–10)**, default **5**. The persisted
`idle_agents_alert` setting is shared between Bus sessions; previously saved
values load again. A changed limit applies immediately in the Settings screen's
session and on the next coordinator tick in other sessions.

A worker counts after 60 continuous seconds shown Idle, with no current or
queued request. Agents with a dialog, pending deletion, or an unknown/launching
status do not count. The orchestrator itself never counts.

When a work room has **more than** the limit, Bus sends one Bus-authored notice
only to that room's bound orchestrator. It names the room, count, limit and idle
agents, and asks the orchestrator to give them parallel work or delete those no
longer needed with `bus agent delete NAME --confirm`. Idle agents use RAM and
require pane polling even when they have no work.

The room stays quiet while the qualified count remains above the limit. Once
it falls to the limit or below, the next crossing sends another notice. Raising
the limit to the current count rearms it; lowering the limit while already over
does not repeat the reminder. Rooms track independent episodes in memory only;
restarting Bus starts fresh episodes after workers qualify as idle again.

A room without an eligible bound orchestrator stays quiet. If one is later
bound while the room is over the limit, it receives the first reminder. Idle
reminders never fall back to the developer or MASTER, and never page Feishu.
