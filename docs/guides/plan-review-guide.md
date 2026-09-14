# Plan Review Guide

A plan records architecture decisions, not line-level implementation. Its job is to tell an implementing agent what is being built, where responsibilities live, and why the chosen boundaries exist. Exact function signatures, control-flow spelling, and code snippets remain implementation judgment unless they define a public contract.

Plan review therefore evaluates macro architecture, module boundaries, requirements, task ownership, dependencies, evidence, and validation. Macro describes the type of finding, not an artificial quota. Round 1 seeks broad macro coverage; Round 2+ is closed-world and reviews only prior findings, the current delta, and direct consequences of that delta.

This guide is language-independent. Rust examples and Bus repository conventions are local adapters, not changes to the review contract.

Archived decisions are outside review scope. They are constraints that the rest of the plan must satisfy. A reviewer may report plan text that contradicts one or an implicit architecture choice that has not been archived, but may not relitigate an archived decision. The developer may request a separate consultation about such a choice.

## Workflow structure is fixed and outside review scope

The workflow has three constants:

1. One plan is one continuous execution. `execute-plan` runs the reviewed task DAG through completion without manual pauses between tasks or waves.
2. Verification binds to the final candidate. Automated or agent-driven end-to-end checks run after all tasks and final gates and before the final landing; the landing must prove the committed tree equals the verified tree.
3. One plan produces one PR, independent of plan size. Tasks and waves are scheduling units, not PR boundaries or manual approval points.

Do not propose findings that insert manual review between tasks, turn waves into mini-plans, move final validation into intermediate tasks, or introduce staged rollout solely to change the one-shot cadence.

A plan that bundles unrelated top-level purposes may be `Abandon` with a recommendation to create independent plans. This splits purposes, not the execution cadence of one purpose.

The goal is locked when plan review begins. Only the developer may expand, shrink, or redefine it. A reviewer may report plan content that fails to serve the stated goal or a goal that is itself multi-purpose, but may not propose a different goal. Non-goals are locked too: never push the plan toward an explicitly excluded direction; report only plan content that contradicts the stated exclusion.

## Review outcomes

| Outcome | Meaning | Suggested plan state | Next action |
| --- | --- | --- | --- |
| Prerequisite failure | Canonical structure or required developer decisions are incomplete | remain in create-plan work | author completes prerequisites and retries |
| `Abandon` | Fundamentally unsalvageable plan | `abandoned` | replace with a new plan |
| `Needs Refinement` | Direction is viable but locally repairable ambiguity, omission, or contradiction remains | `review-plan-in-progress` | author adjudicates and updates, then re-review |
| `Ready` | Current SUBSTANTIVE findings are empty and prior SUBSTANTIVE findings are closed at root cause | `review-plan-complete` | execute the plan |

Prerequisite failure is not a verdict. The only review verdicts are `Ready`, `Needs Refinement`, and `Abandon`. Any current or unresolved prior SUBSTANTIVE finding yields at least `Needs Refinement`; only a fundamental condition yields `Abandon`. Consistency-only drift does not block Ready or trigger a new round. Never issue a conditional verdict such as “Ready after X.”

## Clean Architecture principles

### Separation of concerns

Each layer owns one kind of responsibility:

- domain and service logic contains reusable rules and pure transformations;
- state and orchestration owns lifecycle and interaction transitions without becoming the default home for reusable domain logic;
- views render state and forward user intent;
- infrastructure supplies terminal, process, persistence, transport, and platform capabilities behind explicit boundaries.

Ask whether new logic would still belong in its proposed layer if a second caller appeared. If not, it likely belongs in a lower reusable layer.

### Single responsibility

A function does one job, a file has one theme, and a module owns one coherent responsibility. A unit that mixes persistence, process control, and UI presentation without a boundary should be split.

### Minimal public API

Expose only what current cross-module consumers need. Public Rust items and module re-exports are contracts that become costly to retract. Prefer private or crate-local visibility when no external consumer exists.

### Dependency direction

Higher-level presentation and orchestration may depend on lower-level domain contracts; domain logic must not depend on a terminal view. Concrete adapters implement interfaces defined by the higher-level policy boundary. Validate dependency direction against applicable `AGENTS.md` and repository architecture sources of truth.

### Appropriate dependency injection

Inject dependencies that require shared lifecycle, replacement by platform/environment, or deterministic testing. Do not add a registry or framework for a dependency used at one site with no variant.

### Interface segregation

Prefer narrow interfaces aligned with real callers. Do not create a broad trait whose consumers each use only a small unrelated subset.

### Communicate through contracts, not internal state

Modules exchange defined data, events, and public methods. They do not reach into another module's internal collections or lifecycle state.

### Open for real extension, closed to speculative machinery

Protect established extension points and true variable policies. Do not add an abstraction merely because a future caller might exist.

### KISS

Choose the smallest structure that expresses current behavior and protects real invariants. A one-producer/one-consumer wrapper type, locator object, strategy layer, or framework requires evidence that it reduces current complexity or represents a stable cross-module contract.

Private pipeline carriers may be appropriate when they express a real multi-stage boundary and do not expand public API.

### Fail fast for impossible or unrecoverable states

Defensive fallback is valuable only for reachable failures with a meaningful degraded behavior. For an invariant violation or app-fatal dependency, prefer failure at the nearest state owner. A plan that adds swallowing, placeholder fallback, or retry machinery for an impossible or unrecoverable state should remove it.

This does not weaken error handling for real recoverable failures.

### DRY

Reuse an existing mechanism when purpose, semantics, invariants, and reasons to change are the same. Generalize the existing owner when both old and new callers need the same contract. Do not leave a dedicated implementation in place and add a second “generic” copy. Superficial similarity alone does not justify merging responsibilities.

### YAGNI

Do not build configuration, extension points, state, or failure branches for needs that are not part of the locked goal. Current requirements do not justify an imagined framework.

### Complete the current requirement once

Do not knowingly ship a weaker version of the same in-scope feature and defer its known requirements to a hypothetical later phase. This differs from YAGNI: YAGNI removes imagined future work; this rule prevents under-delivering a stated current requirement. Independent purposes may still become independent plans.

### Minimize change surface

Prefer the fewest files and abstractions that correctly satisfy the goal. Incidental refactors and duplicated adapters expand risk and review cost. Mechanical import or rename fallout should be described as a rule and validated mechanically, not enumerated line by line.

### Task-graph safety

Round 1 must check semantic dependencies and isolation that scripts cannot prove. Every real producer/consumer or hard ordering constraint has a `blocked-by` edge; `consumes` matches upstream `produces`; there are no false edges, hidden cycles, overlapping owners, or tasks that claim independent acceptance while depending on incomplete behavior.

Owned files define exclusive write responsibility and an outer boundary, not a predicted per-file diff. Stable directory ownership may include necessary descendant tests and helpers. Report only real overlap, unowned named paths, unrelated responsibility capture, or ownership that prevents intended safe concurrency.

Every explicitly named changed, added, deleted, configuration, documentation, and test path belongs to the file contract and under exactly one owner. Consumer-fallout candidates must be verified as covered by file contract, owner, and gate or explicitly excluded. A broad owner alone is insufficient.

Scripts detect declared cycles, dangling or duplicate IDs, duplicate names, owner overlap, globs, missing producer edges, and parseable path closure. Reviewers detect omitted semantic edges, false dependencies, wrong boundaries, missing fallout/SOT, and false independent acceptance. The only legitimate response to a cycle is to merge the cyclic tasks and redraw external edges.

### Follow repository patterns

Existing mechanisms and adjacent modules are the default reference. A new pattern requires evidence that the established one cannot satisfy the goal.

## Guardrails

The plan is a set of claims, not evidence.

1. Build an independent understanding from referenced documents, source, tests, and repository architecture before evaluating the proposed solution.
2. Verify every paraphrase against first-party source. Counts, limits, budgets, ordering, lifecycle guarantees, and words such as “always,” “only,” and “never” deserve particular scrutiny.
3. Do not guess. Read the source before claiming that an item exists, accepts a type, or owns a responsibility.
4. Resolve conflicts by authority: source behavior, repository/module SOT, authoritative external documentation, then plan prose.
5. Verify validation commands themselves: they must exist, run in the correct package/workspace, select the intended tests, and prove the claimed property. Exit zero with zero relevant tests is not evidence.
6. Task-local checks stay narrow. Final lint, full tests, coverage, build, and live acceptance belong to the final combined tree. A task-local filtered command cannot substitute for final integration evidence.
7. Reviewers are isolated by lane. Never read another reviewer's round artifacts.

## Adversarial posture: skeptical, not contemptuous

Adversarial review builds the strongest counterargument and tries to prove it with first-party evidence. It does not replace an author's unsupported claim with the reviewer's unsupported story. Seek omitted failure modes, weaker tests, simpler counter-designs, hidden coupling, and unstated assumptions. If the counterargument cannot be verified, it is not a finding.

## Blocking versus direct repair

A reviewer stays read-only. This distinction controls the finding and suggested remediation, not an edit during review.

- A deterministic local defect with one unambiguous correction may include exact replacement text for the author.
- A product choice, architecture boundary, conflicting source of truth, changed goal, or missing developer decision is not auto-resolved. Report the evidence and smallest decision surface.
- Template, owner, graph, and command-shape defects generated by `create-plan` should be caught by its precheck. If one survives, report the exact failed contract without inventing a new state.

Direct-repair examples include a missing known caller in the file contract, a task id gap, a `consumes` edge whose producer is already explicit, or a stale path with one verified replacement. Blocking examples include mutually exclusive ownership designs, two unrelated goals, or a requirement whose only resolution changes the locked goal.

## Finding admissibility and the Ready stop rule

A finding is admissible only when all are true:

- it is macro-level and can change execution outcome;
- it is supported by current first-party evidence;
- it is inside the locked goal and does not advance a non-goal;
- it identifies a concrete ambiguity, omission, contradiction, repository mismatch, unarchived implicit decision, or execution risk;
- the plan author can act on it.

Implementation naming, private field layout, exact function decomposition, stylistic preferences, speculative robustness, and exhaustive runtime-state enumeration are not plan findings. If a valid macro finding has an implementation-level repair, the optional remediation may be precise; the repair detail does not make a standalone implementation nit admissible.

Stop when the plan is sufficient for a competent implementing agent to make ordinary local decisions safely. Do not demand a code-level specification.

## Convergence: SUBSTANTIVE versus CONSISTENCY

SUBSTANTIVE findings change architecture, behavior, task safety, validation, or implementation outcome. They block Ready. CONSISTENCY drift changes wording or duplicated descriptions without changing delivery. It belongs in the non-blocking sync list.

Round 2+ accepts only:

1. unresolved prior findings;
2. current delta;
3. direct contract contradictions or regressions caused by the delta.

Do not create new findings against unchanged regions or reopen a rejected root cause under new wording.

## Conditions for Needs Refinement

- Ambiguous responsibility, lifecycle, ordering, behavior, failure handling, or acceptance.
- Missing in-scope requirement, owner, dependency, migration, compatibility rule, cleanup, test scenario, validation proof, or source-of-truth update.
- Internal contradiction between goal, non-goal, decision, design, file contract, task, or test plan.
- Claims that conflict with current source, repository policy, commands, or supported platforms.
- An architecture choice silently embedded in prose rather than recorded as an archived decision.
- A plan that depends on review history instead of being self-contained.
- Another evidence-backed macro defect that fits no earlier category.

## Conditions for Abandon

- The plan or locked goal bundles multiple unrelated purposes.
- The approach breaks an established module boundary or architectural direction and cannot be locally repaired.
- It duplicates an existing capability instead of using or evolving its canonical owner.
- It introduces a real cyclic dependency.
- It places core business behavior in the wrong layer so the design requires fundamental restructuring.

Round count is never an Abandon criterion. A locally repairable finding remains `Needs Refinement` in every round.

## Evidence-oriented finding format

Each finding contains:

- exact plan location;
- a factual observation;
- first-party evidence with path and line where possible;
- the concrete execution consequence;
- optional directly applicable remediation or choices.

Do not attach severity, priority, blocking, adoption, or source-agent labels. Omit unsupported claims and nits instead of downgrading them.
