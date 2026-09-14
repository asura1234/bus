---
name: create-plan
description: Create or rebuild a single-goal Bus implementation plan under plans/ from explicit requirements, current repository evidence, and the canonical shared template.
---

# Create Plan

Turn `$ARGUMENTS` into one reviewable, executable plan.

```text
INPUT  prompt = $ARGUMENTS
OUTPUT plans/<YYYY-MM-DD>-<short-english-slug>.md
TEMPLATE docs/templates/plan-template.md

========== 1. LOCK INPUT ==========

Read skills/AGENTS.md and docs/templates/plan-template.md completely.

date = today in YYYY-MM-DD
slug = prompt condensed to about five lowercase kebab-case English words
path = plans/<date>-<slug>.md
IF path exists
  append -1, -2, ...; never overwrite an existing plan

Populate every metadata placeholder:
  author      = `gh api user -q .login` after confirming gh authentication,
                or the factual local author when GitHub identity is unavailable
  commit_hash = `git rev-parse HEAD`
  branch      = `git branch --show-current`
  created     = date
  status      = create-plan-in-progress
  prerequisites/follow-ups = explicit prompt facts, otherwise 无

GOAL GATE:
Extract one exact outcome from the prompt and conversation. Cohesion, not task
count, defines one goal. If multiple reasonable goals or interpretations remain,
STOP and ask the developer to lock the goal in one sentence. Never invent it.

Copy only developer-explicit exclusions into non-goals. If none exist, write 无.
Label statements as verified current fact, developer decision, hypothesis to
prove, or deferred production work.

========== 2. DISCOVER CURRENT STATE ==========

Read every user-named local document completely. Inspect relevant source,
tests, Cargo.toml, justfile, applicable AGENTS.md, and direct callers and
consumers. Do not plan from prose alone.

Use rg and rg --files for discovery. Prefer existing contracts and patterns.
For cross-platform behavior, keep public contracts aligned and isolate platform
differences behind existing cfg/platform adapters. If a critical named source is
missing or unreadable, STOP and report it.

========== 3. RESOLVE DECISIONS ==========

Resolve choices in this order:
  1. developer-explicit decisions and non-goals
  2. verified source behavior and repository boundaries
  3. the smallest architecture that proves the requested result
  4. established Bus conventions

Every recommendation and archived selection must serve the goal and avoid all
non-goals. Archive an obvious repository-consistent choice with path-backed
rationale. Leave only product-defining, risky, irreversible, precedent-free, or
preference-dependent decisions open.

IF open decisions remain
  write only metadata, goal, non-goals, current state, references, open
  decisions, and archived decisions
  keep status=create-plan-in-progress and completeness blank
  STOP after reporting the choices

========== 4. BUILD EXECUTABLE BODY ==========

When decisions are closed:
  - select XS/S/M/L/XL/XXL/XXXL and set completeness to 100% only after all
    generated sections are complete and consistent;
  - list every expected source, test, config, and documentation path in the
    file contract;
  - give each behavior-bearing source a concise target-language snippet locking
    its public signature, key types, or control flow;
  - exempt config, lock, docs-only, generated, and import-only fallout from the
    per-file snippet requirement, not from ownership or validation;
  - create normally 1-5 coarse tasks, splitting only for disjoint owners or a
    hard produces/consumes edge;
  - give every task a unique sequential id, goal, exclusive owned files,
    blocked-by, produces, consumes, tools, reference implementation,
    constraints, and a [TASK_LOCAL] acceptance gate;
  - make owner boundaries containment-aware and pairwise disjoint; every
    explicit path has exactly one owner; consumes always names a blocked-by
    producer; the graph is acyclic;
  - use real Bus test filters and commands. Exit zero with zero selected tests,
    the wrong harness, or missing claimed evidence is invalid;
  - declare final gates in canonical order: `just lint`, `just test`, and
    optional `just build`.

Do not include imports in illustrative snippets. Do not estimate LOC.

Read docs/guides/consumer-fallout-format.md completely, then run:
  python3 skills/review-plan/scripts/consumer_fallout.py \
    --plan <plan> --output <artifact>
Use relevant high-confidence fallout to close missing files, owners, consumers,
and gates. The inventory is not semantic proof.

========== 5. VALIDATE AND SET STATUS ==========

Copy every immutable template block verbatim. Keep status
create-plan-in-progress while running:
  python3 skills/review-plan/scripts/review_round.py <plan> --check

Fix every deterministic structure error and rerun. Only after PASS, change only
the status to create-plan-complete. Never shift self-generated template, graph,
owner, goal, or file-contract errors onto review-plan.

========== 6. RETURN ==========

Report in this order:
  1. path, status, size when available, one-sentence goal, and every non-goal;
  2. archived decisions, each with all options, the selected option, and basis;
  3. open decisions, each with all options, pros/cons, and recommendation;
  4. reminder: resolve open decisions, obtain review-plan-complete, then execute.

The reported decision sets must exactly match the plan. Do not implement code,
commit, push, or open a PR as part of create-plan.
```
