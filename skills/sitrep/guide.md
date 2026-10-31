# Sitrep evidence and judgment

A sitrep describes the room's actual work, not the agent roster. Use the most
recent reliable evidence and distinguish a task from its PR when linking them.
Room notes and workflow files may lag; reconcile with recent history and current
host state. Do not turn an idle agent, a queued request, an absent PR, a draft,
or a green focused test into an inferred project status. When evidence is
missing or conflicting, say what is known and what is unavailable in Context.
No source is permission to modify it. Do not create persistent collection files,
fetch Git objects, or run commands that change Bus state. The per-room marker is
the only write. Never include secrets or irrelevant private state in the report.

A blocker stops work or waits on the developer; name what must happen. Put
non-blocking concerns under Ongoing, and use `None` alone when there are no
blockers. A task can appear in Blockers and Ongoing when both describe the same
active work. Deferred items are explicitly postponed, not merely incomplete.
Finished lists only work merged or landed in the specified time window: a
merged PR with its link and title, or a landed commit. Ready, approved,
gate-green, and waiting-to-merge work stays under Ongoing with its real status.
If nothing merged or landed, Finished is exactly `None`, with no caveat such as
"none merged, but...". Use timestamped merge or landing evidence rather than an
undated DONE label. Do not repeat older finished items just because they remain
in notes. Include the initial 24-hour
window and any missing source or truncated query in an appropriate Context.

Map an evidenced current phase to status with this precedence: waiting on a
dependency or developer is `blocked`; an active gate or E2E is `test`; active
reviewers are `review`; code or a fix round in progress is `in-development`;
no PR and no code yet is `planning`. A PR's existence alone is not proof of
review or test. If no phase is evidenced, say so in Context and do not guess a
status; name the omitted task and missing phase in another task’s Context until its phase is established. A status is
not a claim that CI, acceptance, or a review passed.
