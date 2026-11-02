---
name: agent-stats
description: Use when logging agent assignments or task outcomes, backfilling PR work statistics, comparing observed role/model results, or inspecting review uniqueness and workflow bottlenecks.
---

# Agent statistics

Read [guide.md](guide.md) and [stats-format.md](references/stats-format.md)
completely. The machine field/type/enum contract is
[stats.schema.json](references/stats.schema.json).
Resolve `script` to this skill's `scripts/agent_stats.py`.

```text
INPUT = assignment | handover | phase boundary | assignment outcome |
        observation | collection/backfill | report

IF recording an assignment:
  Obtain the task key, intended role, agent ID and orchestrator ID from the brief.
  RUN python3 <script> assign --task <key> --role <role> --agent <id>
    --orchestrator <id> --bus <bus-cli> [--message <brief-id>]
    [--worktree <existing-task-worktree>]
  Keep the returned assignment id for assignment-end.

IF replacing an agent:
  RUN python3 <script> handover --task <key> --role <role> --agent <new-id>
    --old-agent <old-id> --orchestrator <id> --reason <reason>
    --bus <bus-cli> [--message <replacement-brief-id>]
  An old deleted agent needs a prior assignment in this store.

IF a task phase begins or ends:
  RUN python3 <script> task-start|task-end --task <key> --agent <id>
    --phase <phase> [--artifact <completed-artifact-path>]
  Only task-end accepts an artifact. Preserve its returned receipt when a
  caller needs a measured wait interval. Read legal phases from --help.

IF an assignment ends:
  RUN python3 <script> assignment-end --assignment <assignment-id>
    --outcome <outcome> --bus <bus-cli>
  Read legal outcomes from --help. The CLI checks stopped-early evidence;
  an outcome label alone is not counted as a proven early stop.

IF refreshing delivery and compaction observations:
  RUN python3 <script> observe --bus <bus-cli> [--task <key>]

IF collecting one PR's recorded artifacts:
  Capture gh pr view JSON with the fields named by backfill's FIELDS contract,
  or use backfill for automatic capture.
  RUN python3 <script> collect --task pr-<n> --artifacts <artifact-directory>
    [--gh <recorded-gh-json>] [--repo <git-checkout>]
    [--workflow <read-only-workflow-log>]

IF backfilling merged PRs:
  RUN python3 <script> backfill --prs <comma-separated-pr-numbers>
    --artifacts-root <directory-containing-pr-N-directories>
    --repo <git-checkout> [--workflow <read-only-workflow-log>]

IF reporting:
  RUN python3 <script> report [--json]

All commands accept --store except validate. Omit it to use the main Git
checkout's .bus/stats, including when invoked from a linked worktree.
For recorded-fixture replay use the command's --state/--pane/--status/
--evidence/--gh-dir options; never manufacture missing observations.

IF a command exits nonzero:
  STOP this import; inspect the named source. Do not hand-edit JSONL or relax
  the contract to recover a result.
ELSE:
  Return the script's output without adding ratings or rewriting statistics.
```

For the real nine-PR cohort example, see the guide. Output paths and JSON
records are local to the ignored store; this skill does not send messages,
modify Bus state, launch providers, or install prompts.
