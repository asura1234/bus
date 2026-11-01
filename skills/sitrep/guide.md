# Sitrep evidence and judgment

A sitrep describes the room's actual work, not the agent roster. Use the most
recent reliable evidence and distinguish a task from its PR when linking them.
Room notes and workflow files may lag; reconcile them with recent history and
computed repository state. Do not turn an idle agent, a queued request, an
absent PR, a draft, or a green focused test into an inferred project status.
When evidence is missing or conflicting, say what is known and what is
unavailable in Context. No source is permission to modify it. Apart from the
collector's origin fetch and the final marker write, do not change Bus or
repository state. Never include secrets or irrelevant private state.

A blocker stops work or waits on the developer; name what must happen. Put
non-blocking concerns under Ongoing, and use `None` alone when there are no
blockers. A task can appear in Blockers and Ongoing when both describe the same
active work. Deferred items are explicitly postponed, not merely incomplete.

Finished is the computed default-branch Git history since the saved marker.
The agent supplies TLDR and context but cannot add, drop, reorder, or rename
its items. Squash-merge subjects retain their PR numbers. Open PRs are likewise
listed mechanically; the agent supplies their current status and context.
Ready, approved, gate-green, and waiting-to-merge work remains Ongoing unless
the computed Git log says its commit landed. Do not infer landing from an
undated DONE label or repeat older commits already inside the marker window.
Without a saved marker, ask for or explicitly derive a starting point; never
invent a time window or baseline.

Map an evidenced current phase to status with this precedence: waiting on a
dependency or developer is `blocked`; an active gate or E2E is `test`; active
reviewers are `review`; code or a fix round in progress is `in-development`;
no PR and no code yet is `planning`. A PR's existence alone is not proof of
review or test. If no phase is evidenced, say so in Context and do not guess
a status; name the omitted task and missing phase in another task’s Context
until its phase is established. A status is not a claim that CI, acceptance,
or a review passed.
