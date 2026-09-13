---
name: gate-and-fix
description: Run Bus's relevant quality gates, diagnose failures from complete evidence, make scoped fixes, and repeat until the requested gate passes. Use when asked to run gates, fix CI, validate a branch, or prepare code for review.
---

# Gate and fix

Read [guide.md](guide.md) before changing code.

1. Inspect repository status and identify the caller's owned diff. Preserve
   unrelated work.
2. Preflight `command -v just` and `cargo nextest --version`. Do not install or
   upgrade host tooling unless that is in scope. When `just` is missing, inspect
   the recipe and run its direct Cargo/Python/Bun command for focused iteration.
   When `cargo-nextest` is missing, `cargo test --locked` can provide local
   evidence but is not equivalent to the repository's full PR gate.
3. Choose the smallest gate that can reproduce the problem:
   - Rust behavior: `just test-one <filter>`.
   - Python maintenance script: `python3 -m unittest <module>`.
   - Integration asset or docs script: run its matching `just` recipe.
   - Formatting/lint: `just lint`.
   - Full pre-PR confidence: `just ci`.
4. Capture the complete failing command, exit code, and first causal error. Do
   not diagnose from a truncated tail or fix downstream cascades first.
5. Establish the root cause before editing. Add or strengthen a regression test
   when the failure is a behavior bug.
6. Make the smallest coherent fix within the owned scope. Do not bypass,
   weaken, skip, or delete a failing check merely to obtain green output.
7. Re-run the focused failure until it passes, then run the directly affected
   neighboring tests.
8. When preparing a PR, run `just ci` from the final tree. If required tooling
   is unavailable, report the exact unrun gate instead of claiming success.
9. If the workflow was explicitly asked to land changes, hand the verified
   changes to `commit-and-push`; otherwise leave them for review.

Stop and ask for direction if the required fix expands product scope, changes a
public contract, or overlaps changes owned by somebody else.
