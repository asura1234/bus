# Review PR Guide

This guide holds only the workflow principles of `/review-pr`. Code-review dimensions, severity, architecture judgment, and nitpicking
boundaries are defined in one place by the [Code Review Guide](../../docs/guides/code-review-guide.md); report fields and order are defined in one place by
the [Review Format](../../docs/guides/review-format.md).

## A lane is one independent viewpoint

Only when the same reviewer uses a stable lane across rounds can it reconcile its own history. When multiple models review in parallel, each reviewer
uses a distinct and stable `--reviewer <name>`, and none reads another's `review.md`. The value of different models comes from independent
viewpoints, not from three reports finding the same problems; the author merges legitimate overlap by location and semantic root cause.

When unambiguous, a bare invocation uses the `default` lane. Concurrent bare invocations may be temporarily isolated by the script into `default-2`, `default-3`;
once multiple or named lanes exist, you must pass back the same reviewer name the script lists.

## Lock the goal; do not change the assignment for the developer

With an associated plan, `## 目标`, `## 非目标`, and `## 已归档的决策` are review inputs, not content for the reviewer
to redesign. Without a plan, the goal must come from the developer's one-sentence answer and cannot be guessed from the diff, commits, or PR description.

A reviewer may report an implementation that deviates from the goal or archived decisions, but may not demand an expanded goal, a move toward a non-goal, or
overturn an archived decision with its own approach. Only when new source evidence truly disproves a decision's factual premise is the challenge escalated to the developer.

Before investigation, proof, delegation, and output, uniformly apply code-review-guide "goal-relevance admission". Dimension 6 is the only exception:
it must write out-of-goal diff as a scope comment by cumulative slice size, but does not authorize deep review of that slice. `XS` / `S` go into the sync list,
`M` and larger go into new problems, recommend `split-pr`, and yield the split Abandon; a Good Samaritan inclusion permit adds no follow-up repair obligation.
Diff coverage and round boundaries can only further narrow the scope of other dimensions.

## The plan is input, not a code-review target

Committed changes under `plans/**` are all excluded from code-review scope; they do not enter the diff snapshot,
delta, touched-file set, findings, sync list, or PR single-purpose judgment. Only an explicitly associated `--plan` is read,
and only as input for locked goal, non-goals, and archived decisions; the reviewer does not review,
comment on, or demand fixes to the plan file itself. When the branch has no other committed changes after excluding `plans/**`,
the `review-pr` prologue should fail for having no reviewable code.

## Round 1 establishes coverage; Round 2+ consumes the delta

Round 1 first identifies the diff's relationship to the goal, reads every eligible touched file in full, checks all 9 dimensions, and records uncovered areas explicitly in the ledger.
Upstream/downstream reading serves goal-related judgment and does not expand into a general audit of touched modules.

With `--scope`, the assignment is a fixed set of whole committed files, including unchanged production
code. The production-review Goal admits all their contents; Round 1 covers all 9 dimensions there.
The canonical file list and exact test allowlist identify the chunk, independently of manifest path
or ordering. Branch and chunk histories, and histories of different chunks, stay isolated. Round 2+
uses this lane's prior whole-file snapshot and the author's committed per-file delta; rebases or
uncommitted reviewer probes alone do not produce a delta. Direct dependencies remain read-only context,
and findings reference chunk files. The same closed-world rule below applies to both target kinds.

Round 2+ is a CLOSED WORLD:

- first check goal relevance of open prior-round problems, then reconcile;
- identify and review goal-related changes in this round's delta;
- check upstream/downstream contract contradictions or regressions directly introduced by the delta;
- cover an area once only when the ledger explicitly proves Round 1 missed it.

Incremental rounds do not rescan covered, unchanged code and do not rely on fresh territory to sustain finding counts. Convergence comes from monotonically
shrinking scope, not from a hard cap on rounds.

## Write behavioral suspicion as a test first, then as a finding

**Scope**: test proof targets only **claims about runtime behavior** — dimensions 2 correctness, 3 security, 7 bug.
Other dimensions are judged by reading source; evidence lists only `path:line` and the source observation, writes no probe, and does not mark
`已证明` / `未证明`. All proof, investigation, and output first pass code-review-guide "goal-relevance admission",
then this round's coverage boundary; dimension 6 is the only exception, doing only purpose / cumulative-size classification on out-of-goal diff.

**A default action, not an option**: write every behavioral suspicion as a test first, then decide whether to write it as a finding. A red test simultaneously
gives the root cause, reproduction, and repair acceptance criterion, none of which a paper suspicion has; a behavioral suspicion that cannot be written as a test usually
has not been thought through. Run it narrowly (Rust with `just test-one <filter>`, a skill's Python tests with
`python3 -m pytest <test file>::<test name>`), with scope locked to the area of this suspicion;
not limited to tests you added — related existing tests may run too. Quality gates — lint, format check, coverage,
build — and the unfiltered full test suite all belong to CI and `gate-and-fix`; the reviewer does not run them.

**Write access covers only test files**: you may create test files or add your own cases to existing test files. Production code,
config, build scripts, and docs are never touched; adjusting the implementation to make a test red manufactures evidence, and the hypothesis fails.
In scope mode those writes are further limited to the exact `SCOPE_TEST_FILES` allowlist, including
declared new dedicated test files; a missing harness connection does not authorize production wiring.
Do not write tests to pad numbers either — a probe must have a meaningful failure mode and turn red when the implementation is wrong; asserting the harness's own output
or a just-configured mock is not proof.

**Probe lifecycle**:

- probes stay in the working tree, uncommitted, and the author decides to adopt, rewrite, or delete them; tests should be keepable
  long-term as normal regression tests;
- file and case names follow code-review-guide "probe test naming and ownership"; first make clear which
  files you may write, never overwrite or delete someone else's probe, never delete or weaken existing assertions; lane / round are recorded only in the review
  artifact, not in test names;
- red → write a finding in the corresponding dimension, with the first evidence item
  `已证明：<test name @ relative test file path> — <failed assertion>`;
- green → withdraw the suspicion and open no finding; the probe stays in the working tree, with one line recorded in review.md
  "本轮探索区域 / 运行的测试";
- never leave only a red test without writing a finding.

**What may stop at "unproven"**: whatever Unit / Integration tests can prove must be proven before reporting,
never reported as a suspicion. Only behavior that "can be confirmed only by manual verification in a real terminal session"
may stop at suspicion: the first evidence item is `未证明`, with no explanation and no list of what was tried; phrase it as "cannot rule out X", and
write the impact conditionally. `未证明` does not block Ready, does not trigger a new round, and cannot alone justify changing verified existing behavior.
(Full criteria in code-review-guide "verification boundary" and guardrail 5.)

A review round's quality is measured not by finding count but by how many behavioral suspicions received goal-related evidence or were ruled out;
do not reward continued pursuit by number of red tests.

## The triage ledger is the memory of author dispositions

`TRIAGE_LEDGER` must be read in full and applied oldest to newest. A newer disposition of the same root problem overrides an older one.

- `rejected`: do not reopen under new clothing; challenge only when new evidence disproves the factual premise.
- `applied`: first recheck goal relevance, then verify it was really fixed; out-of-scope items close per the admission rules, and a prior APPLY does not modify the goal.
- `flagged`: the developer is deciding; do not copy it into another finding.

This keeps different lanes independent while never repeatedly asking the author to handle the same already-decided thing.

## Fan-out increases coverage, not report count

Subagents are a narrow division of labor inside one reviewer lane, not extra reviewers. Choose the fewest necessary
dimension groups by the actual diff; small diffs do not parallelize for parallelism's sake.

Subagents may return only raw candidates. The main agent must go back to real source to verify, deduplicate, discard nitpicks, and produce
the single `review.md`. All subagents are released before the report is produced; the next round decides afresh from the delta whether dispatch is needed.

Fresh-eyes and platform built-in review serve as supplementary candidate sources only in full rounds; they cannot replace guide-driven
review, nor reopen covered areas in incremental rounds.

### Fresh-eyes is the source of viewpoint; dimension groups are the source of coverage

Dimension groups set out with the same checklist and find what the checklist anticipates. Problems the checklist cannot name
are bumped into only by an agent that has not read it — so fresh-eyes is not a nice-to-have supplement; it is this round's
only viewpoint not framed by the checklist, and it must be dispatched.

For the same reason, it is invalidated once it reads the **dimension list**: an agent that has read the list goes confirming the list item by item instead of looking at the code.
The ground truth it receives must be verbatim identical to the other groups, with the only difference falling on "no checklist" —
otherwise there is no telling whether a difference it finds comes from viewpoint or from input.

What is forbidden is the checklist, not discipline. It must still read this guide's "Write behavioral suspicion as a test first, then as a finding" section:
the proof boundary is written only there, and the next paragraph describes exactly why it most needs that boundary.
code-review-guide, architecture-principles, the dimension list, and any prior-round review are never read.

But "no checklist" means **it decides what to look at**, not that "it decides what counts as proof".
It is at once this round's highest-value and highest-variance source: most likely to find real problems the checklist does not cover, and also most likely to produce
the broad, confident speculation — and the latter is exactly what turns review into a second source of entropy. The proof boundary is not loosened by a word.

## Adversarial posture raises proof pressure without lowering fairness

`--devils-advocate` means actively seeking counterexamples and verifying claims outside the code, while still bound by the same goal, scope, proof standard, and
readiness rules. Suspicion is not default rejection; a counterargument also needs evidence. Switching posture creates no new lane,
nor turns an incremental round back into a full round.

## A finding is evidence, not a sales pitch

Reports must be agent-anonymous. A finding states only location, observation, evidence, impact, and optional remediation, never writing
"blocking", "recommend adopting", P0/P1, or the source model. Severity is used only to compute the single final verdict.

Do not write the exploration process, praise, first-person retrospectives, or "what else should I do". With no substantive problem, `新问题与建议`
is strictly `无。`; candidates tried but not upheld enter at most the file-only `本轮探索区域`.

The sync list usually carries wording drift; dimension 6 `XS` / `S` out-of-goal code slices are an explicit exception. The "other" dimension still
must satisfy formal goal relevance and materiality admission and cannot shelter implementation details, style nits, or candidates excluded by other rules.

The full prior-round reconciliation table stays in `review.md` as the audit record; chat shows only the one-sentence summary generated by the renderer.
`本轮探索区域` likewise serves only later reviewers and does not enter chat. Other sections are preserved verbatim.

## The renderer enforces the output protocol

The agent's responsibility ends at writing a legal, complete `review.md`. The chat body is validated and deterministically generated by
`cli_extensions/review_artifact.py render-response`:

- validates the title, fixed sections, order, ledger, and verdict;
- removes the file-only `本轮探索区域`;
- condenses the full prior-round reconciliation table into its existing summary;
- preserves all other content verbatim.

So the skill no longer asks the model to hand-execute the same "output discipline". The final reply returns the renderer output verbatim.
