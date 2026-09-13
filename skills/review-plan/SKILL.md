---
name: review-plan
description: Perform an iterative, read-only review of a Bus implementation plan, preserving a stable reviewer lane and checking architecture, completeness, task ordering, and validation. Use when asked to review or re-review a plan.
---

# Review plan

Read [guide.md](guide.md) before reviewing.

Input is a plan path and optional reviewer name. If no path is given, select the
single most recently modified file under `plans/`; if selection is ambiguous,
ask for the exact path. A named reviewer remains the lane for every follow-up.

## Boundary

- Do not modify the plan or repository, stage files, commit, stash, checkout, or
  rebase.
- Read-only inspection and focused commands that verify plan claims are allowed.
- Review artifacts may be written only under
  `temp/review-plan/<plan-slug>/<reviewer>/`.

## Round 1

1. Read the entire plan, stated references, relevant implementation, tests,
   `Cargo.toml`, `justfile`, and repository policy.
2. Lock the proposed goal and non-goals. Do not expand the product because a
   different design seems attractive.
3. Trace each requirement to design, implementation task, and validation.
4. Check data/control flow, ownership, persistence, concurrency, cleanup,
   error handling, compatibility, and security where applicable.
5. Check the task graph: dependencies must precede consumers; independent work
   must not edit the same state without a merge owner; every behavior change
   needs a testable acceptance signal.
6. Verify file paths, symbols, commands, and external assumptions against the
   current repository. Mark inference as inference.
7. Report only actionable findings. For each: severity, claim, evidence,
   consequence, and the minimum plan correction.

## Follow-up rounds

Read the previous review and author response. Review only unresolved findings,
the plan delta since that round, and direct consequences of the delta. Do not
reopen equivalent nits or expand to unrelated pre-existing design.

## Verdict

- `Ready`: no material finding remains.
- `Not Ready`: at least one material finding remains.

Write `review.md` in the artifact directory, then return that content. Keep the
verdict evidence-backed; a clean format is not proof of correctness.
