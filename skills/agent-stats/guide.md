# Interpreting objective agent statistics

The questions are which observed results differ by role/model, and where work
waits. The script measures work; it does not assign ratings. Keep workload,
sample sizes and missing coverage visible when comparing agents or models.

An assignment records intent from the orchestrator. Bus state supplies the
unique ID/name and provider; identity-matched, session-verified native footers
supply model labels. Unsupported or ambiguous surfaces produce `unknown`.
Current pane data is never retroactively assigned to a historical review.
Hand-written model annotations in a review lane are not model evidence.
For an orchestrator's own coordinating task, use its ID for both selectors;
its work-room binding is checked even though its home room is MASTER.

Use `pr-N`, the numeric PR number, or the exact GitHub branch name as the
assignment task key to link author assignments to merged PR facts. Only
assignments observed before the merge qualify. Authored PR counts describe
participation; shared authors each receive one participation count. Completed
rebase receipts supply old/new/base SHAs and replayed commits; workflow mentions
remain timeline context and do not count as completed rebases.

For reliable forward attribution, end a review or validation phase with its
actual artifact path. That binds its exact content hash to the assignment's
identity and an automatically observed completion time. Reruns that produce
new content require their own phase/artifact binding. A calendar date in a
historical review does not establish ordering within that day.

The canonical author triage ledger owns root-cause deduplication. A source
assertion, an adopted probe, a repeated finding, and a shared root are different
facts. The collector reads existing canonical decisions and reporting lanes;
it never merges findings by titles, files, embeddings, or its own judgment.
Reviewer evidence copies of an author's ledger are not additional triage runs.
Precision is APPLY / (APPLY + REJECT); FLAG and WITHDRAWN stay separate. Missing
source IDs or rounds are coverage gaps. Severity is unavailable because the
review artifact format prohibits finding-severity labels.

An escaped defect requires an APPLY root, ordered Ready/detection evidence,
Git ancestry, and unchanged cited blobs. This conservative proof deliberately
omits cases where code moved or context changed. A changed-file count after
Ready is a tree delta, not an escaped defect; rebased upstream changes may be
present in that delta.

Assignment outcomes are the orchestrator's recorded end states. A proven
stopped-early count additionally requires an ended request without a captured
reply, observed Idle, a recorded repeat assignment or stopped handover, covered
home-room history without an authored result message, and an unchanged tracked
task branch and PR set. Supply `--message` and `--worktree` when assigning work
whose delivery can be tracked that way. Without the baseline or coverage, the
end state is retained but the early-stop metric is unproven. A later unrelated
message also makes the conservative no-result check unproven. No provider is
presumed to stop early.

The store uses local POSIX advisory file locks.

Cumulative compaction counters are per agent, as observed from Bus. They cannot
be attributed to a past model or role from a current snapshot. Delivery stages
and reasons are retained verbatim; repeated observations of one request do not
increase failure counts. A gate wrapper and the runner report it explicitly
references count as one gate; both source artifacts remain available. Phase durations are computed from paired clock
observations; an open phase has no completed duration. Parallel lane timings
can overlap and must not be added to claim wall time.

Records retain source hashes. Collection copies original input bytes into the
ignored store so worktree deletion does not erase the proof. Re-importing the
same inputs is idempotent. Current gate artifacts use the sibling gate-and-fix verifier; schema-2
archives have a dedicated metadata adapter. Review conflicts and malformed known formats stop
an import. A backfill commits records per PR; its successful summary lists the
complete imported cohort. Keep the store private: native panes and raw review
artifacts may contain work context.

Example for the requested cohort (run from the repository, with this skill's
script path resolved):

```sh
python3 "$script" backfill --prs 284,285,287,288,289,290,291,294,295 \
  --artifacts-root /Users/dylanliu/work/bus/temp \
  --repo /Users/dylanliu/work/bus \
  --workflow /Users/dylanliu/.local/share/bus/sessions/585d051bcf3a8551/workflows/bus.md
python3 "$script" report > temp/agent-stats/report.md
python3 "$script" report --json > temp/agent-stats/report.json
```

Those commands read the workflow and original PR artifacts. They write only the
local statistics store and the caller's report redirections. Historical work
start times, missing author assignments, missing model identities and failure
classifications remain unavailable. Gate runner instrumentation is a separate
add-on to the test runner; this skill already accepts its measured fields.
