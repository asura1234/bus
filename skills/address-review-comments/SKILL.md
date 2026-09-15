---
name: address-review-comments
description: Adjudicate and address review comments for plans, PRs, tasks, or free-form review. Deterministically sanitize inputs, atomize and deduplicate claims by root cause, obtain first-party evidence for each claim, decide APPLY, REJECT, FLAG, or HOUSEKEEPING centrally, then remediate related accepted claims together. Plan and PR modes land through commit-and-push by default; task mode requires --no-commit-and-push. Use when asked to address, respond to, or fix review feedback.
---

Use this workflow for `review-plan`, `review-pr`, execute-plan task review, or free-form review. Reviewer and author are peer decision-makers in a convergence loop: the reviewer identifies a problem and may suggest a repair; the author independently determines whether the claim is true, whether it belongs in scope, and what repair is correct; a later review verifies the outcome. Review comments are evidence-bearing claims, not commands.

Before execution, read completely:

- [Review Response Guide](../../docs/guides/review-response-guide.md), the source of truth for verification and dispositions;
- only the mode-specific judgment sections named by that guide from `plan-review-guide.md`, `code-review-guide.md`, or `task-review-guide.md`. Those are reviewer-side documents; do not import their full review-production workflow or issue a reviewer verdict from the author side.

```text
INPUT = [plan | pr | task] [--round latest|N] [--review-file <path>]...
        [--free-form-file <path>]... [--label <name>]... [--no-commit-and-push]

HARD RULES
- Modify the repository only for claims finally adjudicated APPLY. REJECT, FLAG, and HOUSEKEEPING never change repository content.
- The main agent exclusively owns deduplication, cross-claim comparison, disposition, triage, remediation grouping, and landing.
- Evidence gathering answers a bounded factual question only. It does not decide disposition, propose a patch, or modify the repository.
- Claim truth, goal/scope admission, and repair design are separate decisions. SUPPORTED does not approve the requested scope or the reviewer's proposed fix.
- The main agent chooses whether to verify serially, delegate read-only bounded fact questions, or mix both according to claim count and shared context. The workflow does not prescribe parallelism.
- No writes occur until every canonical claim has first-party truth evidence and the main agent has completed one unified adjudication barrier.
- Run only diff-scoped validation in this workflow. Full repository gates belong to gate-and-fix.
- Plan and PR modes land through commit-and-push by default. Task mode requires --no-commit-and-push and changes only its owner scope.

========== PASS 0: RESOLVE AND SANITIZE ==========

Read(docs/guides/orchestrated-room-brief.md) completely, then run:
  python3 cli_extensions/room_assignment_context.py [--frame "<frame>"] \
    --output temp/address-review-comments/room-assignment-context.json
IF exit != 0:
  reproduce stdout verbatim as blocker evidence and STOP.
ORIGIN = ASSIGNMENT_ORIGIN

IF ORIGIN == verified:
  the locked goal and non-goals are the context file's goal and non_goals; every lane's
  locked goal and any plan Goal/Non-goals must match them exactly, otherwise STOP with blocker evidence.
  IF the Orchestrator's assignment explicitly authorizes plan-review finalization with a plan hash and lane rounds:
    read the review response guide section `Plan-review finalization boundary`, then run:
      python3 skills/address-review-comments/scripts/finalize_plan_review.py finalize \
        --plan <plan> --review-root temp/review-plan/<branch_slug>/<plan_basename> \
        --expected-plan-hash <hash> --lane <lane>:<round> [--lane <lane>:<round>]...
    return its receipt or FAIL output verbatim and STOP.
IF ORIGIN == NotInBusRoom:
  the caller or developer supplies the locked goal exactly as below.

IF --review-file or --free-form-file is explicit:
  canonicalize in argument order and deduplicate.
ELSE:
  only legacy plan/PR lane discovery is allowed;
  task mode requires explicit mode and --review-file.

Run:
  python3 skills/address-review-comments/scripts/prepare_review_input.py \
    [--review-file <path>...] [--free-form-file <path>...] [--mode plan|pr|task] \
    [--label <name>...] --output <run-root>/sanitized.md

IF exit != 0:
  STOP. Never bypass the sanitizer by reading the original review directly.

Legacy lane discovery follows the review skills' branch slugging and round rules. Use the requested round or the latest valid round per lane. Explicit mode limits discovery to that mode. If both plan and PR lanes exist, or neither exists, STOP.

Review files created before this migration are immutable historical evidence,
not canonical structured artifacts. Supply one with `--free-form-file <path>
--mode plan|pr`; the sanitizer quarantines its full text and preserves
provenance. Never rewrite a historical review in place merely to satisfy the
new parser.

Read only `新问题与建议` plus the complete `同步清单（CONSISTENCY drift，非阻塞）` from sanitized output.

- Structured lanes must agree on mode, target, base, locked goal, plan/task identity, and task SCOPE_HASH when applicable. Any mismatch stops the run.
- Free-form input requires explicit mode. Its round is `n/a`. Plan/PR SCOPE_HASH is `n/a`. Locked goal must come from the caller or developer and cannot be inferred from diff, commits, or PR prose.
- Free-form input has no mechanical round provenance, so closed-world gate (d) cannot be evaluated. Record exactly one triage line noting that gate (d) is indeterminate for free-form input.
- Plan mode reads the complete plan and archived decisions. PR mode reads `<base>...HEAD`, locked goal, optional plan context, provenance, complete claim-relevant files, and direct dependencies. Task mode reads only the task contract, snapshot, owner, SCOPE_HASH, and gate evidence.
- Task mode requires --no-commit-and-push. Plan/PR mode delegates landing to commit-and-push; direct `master` landing is legal only when the developer explicitly requested it.

Run root:
  plan/pr = temp/address-review-comments/<slug>/<YYYYMMDD-HHmmss>/
  task    = temp/address-review-comments/__task__/<full-plan-slug>/<task-id-name>/<timestamp>/

IF mode == pr:
  identify reviewer-created test files and newly added cases in existing tests;
  record their source claim and original red/green state;
  remain read-only during this pass and do not classify them as unrelated merely because they are uncommitted or red.

========== PASS 1: ATOMIZE AND DEDUPLICATE (main only) ==========

1. Extract substantive findings and sync-only assertions. Record source lane, round, file, identifier, location, dimension, observation, evidence, and impact. Keep suggested repairs as non-binding candidates separate from the claim.
2. Split a finding when it contains independently true factual claims. Every source assertion maps to exactly one canonical claim.
3. Deduplicate across lanes by actual root cause and retain corroborating sources. Mark mutually exclusive suggestions only as candidate conflicts.
4. Record for each canonical claim: claim ID, source assertions, the narrowest context pointer available, and one atomic factual question to verify.
5. Atomize by independent truth, not maximum granularity. Claims sharing root cause, file, and repair site remain one claim.
6. An empty claim set is valid and continues through deterministic zero-count output.

========== PASS 2: VERIFY EVERY CLAIM ==========

Every canonical claim receives one first-party truth assessment:
  SUPPORTED | CONTRADICTED | INCONCLUSIVE

Include the evidence location and uncertainty. If evidence is unavailable, say so; do not guess. The assessment covers only the problem claim, not the reviewer's proposed repair.

For PR claims, establish behavior owner, goal relationship, and concrete consequence. Verification is bounded by the guide: never skip solely because of a title or path, and never expand into an unrelated subsystem audit or repair design.

The main agent decides whether to verify directly or delegate. Any delegated verifier is repository- and Git-read-only, receives the locked goal and one bounded fact question, reports first-party evidence plus only the closed truth enum, and does not decide disposition, propose remediation, modify files, or discover extra issues. One verifier's report cannot be used as another verifier's evidence.

Chat summaries are not evidence. Every assessment cites a path:line, exact section, or command and output. Any unresolved STOP blocks adjudication and remediation.

========== PASS 3: ADJUDICATE (main only) ==========

For every canonical claim, the main agent:

1. verifies that evidence is first-party, relevant, and sufficient;
2. applies the guide's goal-and-scope gate before any repair decision;
3. compares candidate conflicts and resolves them only from first-party facts;
4. chooses exactly one guide-defined disposition mapped to APPLY, REJECT, FLAG, or HOUSEKEEPING;
5. independently derives the repair for every APPLY claim from current context, root cause, behavior owner, required invariants, and regression evidence. The reviewer's suggestion is only a candidate and may be used, changed, or replaced. If a safe repair still requires an open product or architecture choice, use requires-developer-decision instead of forcing the suggestion.

For PR mode, a supported but out-of-goal repair request is always REJECT(scope-change). A red test, multiple lanes, prior APPLY, or incidental-work permission does not make it in scope.

Write <run-root>/triage.md using the guide's fixed sections and citations. Each disposition must be supported by first-party evidence. APPLY records the author's independently selected repair direction, not copied reviewer instructions.

========== PASS 4: GROUP AND REMEDIATE ==========

In plan mode, preserve the canonical state transition: create-plan-complete -> review-plan-in-progress. Existing review-plan-in-progress or review-plan-complete does not move backward. Never return to create-plan-in-progress unless a new developer decision genuinely requires it.

Group APPLY claims by related root cause, touched files, and dependency order. Claims sharing a root cause, file, or dependency belong to one group. Compute an exact file allowlist per group. Implement the independently derived repair, not the reviewer's wording as a task specification.

For PR mode, include accepted reviewer tests or test hunks in the allowlist. Run those tests and narrow direct regressions after the repair. Landing must include every adopted new test, including untracked files.

The main agent implements serially when groups are small, overlap, or are trivial. Writers are justified only for materially independent groups with disjoint allowlists and no dependency. Plan mode has one writer for one plan. Task mode has one owner and never lands.

After delegated writers return, the main agent verifies actual status against the union of allowlists, unique file ownership, real green evidence, and no reopened FLAG or conflict. Unresolved red or out-of-scope changes are handled centrally or converted to FLAG; never use destructive rollback.

Run only validation scoped to changed packages, files, and exact tests. Record every full gate that was not run. Full lint, full tests, coverage, build, and live acceptance belong to gate-and-fix on the final committed tree.

Serial plan/PR work invokes commit-and-push after scoped validation. Parallel work invokes it once after every group is reconciled. --no-commit-and-push forbids add, commit, and push.

========== OUTPUT ==========

Report mode, target, lanes/rounds, verification orchestration, disposition counts, cross-lane merges/conflicts, changed files, exact scoped validation, explicitly unrun full gates, landing/no-landing status, and triage path. For parallel remediation, also report groups and allowlists.

Next action:
  plan -> rerun review-plan
  landed PR -> may run pr
  --no-commit-and-push -> state explicitly that nothing was pushed
  task -> return to execute-plan

REJECT and FLAG are separate:
  no REJECT -> one line exactly `REJECT：无`
  otherwise -> one line per rejected claim with disposition, source, and necessary first-party contradiction

FLAGGED is the final output content:
  no FLAG -> one final line exactly `FLAGGED：无`
  otherwise -> each flagged item includes evidence, why a decision is required, and options without deciding for the developer
```
