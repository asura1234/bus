# Idle agents alert

Settings includes **Idle agents alert (1–10)**, default **5**. The persisted
`idle_agents_alert` setting is shared between Bus sessions; previously saved
values load again. A changed limit applies immediately in the Settings screen's
session and on the next coordinator tick in other sessions.

A worker counts after 60 continuous seconds shown Idle, with no current or
queued request. Agents with a dialog, pending deletion, or an unknown/launching
status do not count toward the first reminder. The orchestrator itself never counts.

When a work room has **more than** the limit, Bus sends one Bus-authored notice
only to that room's bound orchestrator. It names the room, count, limit and idle
agents, and asks the orchestrator to give them parallel work or delete those no
longer needed with `bus agent delete NAME --confirm`. Idle agents use RAM and
require pane polling even when they have no work.

The room stays quiet while the qualified count remains above the limit. Once
it falls to the limit or below, the next crossing sends another notice. Raising
the limit to the current count rearms it; lowering the limit while already over
does not repeat the reminder. For rearming, Idle workers of any age and workers
temporarily Unavailable or Launching still hold the episode open, as in the
original alert: losing a status reading is not evidence of new work. Rooms
track independent episodes in memory only;
restarting Bus starts fresh episodes after workers qualify as idle again.

A room without an eligible bound orchestrator stays quiet. If one is later
bound while the room is over the limit, it receives the first reminder. Idle
reminders never fall back to the developer or MASTER, and never page Feishu.

## One agent idle longer than the minutes setting

`idle_alert_minutes` is a second Settings value, **Idle alert (minutes, 1–60)**,
default **5**. It does not replace the spare-count limit above. Both notices
are posted by the same `notify_idle_agents` path.

When one work-room agent has been shown Idle, with no current or queued
message, for **more than** that many minutes, Bus sends one notice only to
that room's bound orchestrator. The text names the agent id and name and says
the agent has been sitting idle longer than the threshold. MASTER orchestrators
never qualify. Working, blocked (including a dialog), launching, unavailable,
and pending-deletion agents do not qualify.

Working, blocked (including a dialog), a current or queued message, or a
completed message newer than the saved start resets the clock to the later of
the status change and that message. Unavailable and Launching, including the
restart poll gap, do not. The same episode does not notify again. After the
agent is active and then idle again, the new episode may notify once. The
episode start is saved on the agent, and the notice records that same start, so
a restart neither repeats a notice nor forgets time already idle. An episode
that crosses the threshold only after restart still notifies once.
Changing the minutes value does not repeat an episode that already notified.
A room with no bound orchestrator stays quiet until one is bound.
