# Plan Review Guide

A plan document records **architecture-level decisions**, not an implementation-level code specification. Its role is to give the implementing agent a clear understanding of: what to build (functional boundaries), where to build it (module and layer division), and why it is done this way (decision background and constraints). As for how — the exact parameters of a function signature, the detailed implementation of control flow, the precise wording of a code snippet — the implementing agent is capable of judging for itself and does not need the plan to dictate it.

**A plan is not a literally compilable blueprint.** During implementation the implementing agent reads the codebase, understands the context, and handles implementation details on its own. The value of a plan lies in fixing the architecture decisions so the agent does not pick the wrong direction at key forks — not in replacing the agent's engineering judgment.

**The goal of review is therefore**: whether the architecture decisions are clear, whether module boundaries are explicit, whether the key "what to build" is unambiguous — not checking whether every code snippet is precise down to parameter types. Code snippets in a plan illustrate intent; they are not templates to be copied verbatim. If a piece of pseudocode lets the agent understand the intent, it is already enough.
Plan review makes only **macro-level** judgments (architecture, modules, boundaries, decisions), not function-level / code-style nits. Note: macro limits the **type** of finding, not the **number** — ambiguity, omission, internal conflict, inconsistency with the codebase, test-scenario gaps, and execution risk points are all in scope. But **"exhaustive" is the breadth goal of Round 1, not something to grind every round**: **Round 1** covers macro-level risk as completely as possible; **Round 2+** converges under the closed incremental rule (only reconcile prior rounds + review this round's diff and its direct consequences), and **no longer digs into unchanged regions** (see "Convergence: SUBSTANTIVE vs CONSISTENCY"). In Round 1, a low finding count is meaningful only when coverage is wide enough; in incremental rounds, a low finding count is exactly the normal sign of convergence — do not misread it as "not reviewed enough" and go forcing new issues out of unchanged regions.

This guide is language-independent: the principles apply to any target language such as Rust, TypeScript, Swift, or Kotlin. Where concrete syntax is involved, follow the target language's conventions; examples in this document are mostly expressed in Rust (this repository's main language), and the equivalent forms in other languages follow likewise.

**Archived decisions are outside review scope**: `已归档的决策` records the established choices the developer already made while writing the plan. Review rounds (`/review-plan`) **do not evaluate or relitigate** these decisions — they are constraints the rest of the plan must be consistent with, not objects of review. When the developer wants an opinion on a decision, they **manually and separately** consult an agent, outside review rounds; the following dimensions serve as the evaluation baseline for such a consultation:

- **Architecture consistency**: Is the decision consistent with the existing architecture patterns and module boundaries in the codebase? It must be confirmed by reading the relevant module SOT (an applicable `AGENTS.md` records a module's responsibility, dependency direction and public API; cross-module conclusions follow the repository's applicable architecture sources of truth)
- **Clean Architecture compliance**: Does the decision meet the requirements of the [architecture principles](architecture-principles.md)? (separation of concerns, dependency direction, single responsibility, etc.)
- **Simplicity**: Is there a simpler alternative? (KISS, YAGNI)
- **Consistency**: How are similar problems solved in the codebase? Does the decision follow the existing pattern?
- **Duplication**: Does the decision introduce an implementation that duplicates an existing mechanism? (DRY)
- **Technology choice**: Do the frameworks, libraries or platform APIs the decision introduces satisfy the three pillars — **latest** (the platform-recommended modern approach), **most popular** (high community adoption, actively maintained), **best documented** (thorough official docs, wide community coverage)? A stack all three pillars point to is also the most stable foundation for AI agents to generate high-quality code.

Within a review round, only two decision-related things remain in scope:
- Plan body **inconsistent** with an archived decision (internal conflict) → finding
- **Implicit decision**: an architecture choice silently made in the body but not archived → ask the author to archive it explicitly (only require recording it, do not judge the choice itself)

Note: review does not judge pure style preferences (naming style, code organization, etc.); it only cares about architecture-level soundness. If several approaches are all architecturally sound with little difference, do not demand a change merely on subjective preference.

## Workflow structure is a fixed constant, outside review scope

This repository's plan workflow has three **structural constants**. They are the process skeleton fixed by the team, not optional design of the plan author, so the reviewer **does not evaluate or suggest changing them**:

1. **One plan = one-shot execution**: the reviewed task DAG runs through all tasks continuously wave by wave, with **no** manual confirmation / pause / gate inside the execution. Plan size does not change the one-shot contract.
2. **Verification binds to the final commit**: agent-driven e2e may run only after all tasks and the automatic EXIT CHECK and before the final `commit-and-push`, against the plan delta that no longer changes; after committing it must be proven that `HEAD^{tree}` equals that verified delta. Manual testing targets only the plan's final commit, stays in its own section, and must not be stuffed into task blocks or between waves.
3. **One plan = one PR** (regardless of size): tasks/waves are parallel and dependency scheduling units of the same execution, not independent PRs or manual breakpoints.

The reviewer is **forbidden** to raise any finding that would change this structure into "per-task manual testing / pauses between waves", including but not limited to:

- Suggesting the one-shot execution be split into "do A first, then do B after manual confirmation / manual testing", inserting a manual gate / pause / mid-way acceptance between tasks or waves
- Suggesting the tasks be turned into a series of "run, stop, check" mini-plans because the plan is "too big"
- Suggesting manual or e2e verification be moved earlier, or stuffed into the middle of implementation tasks
- Suggesting gradual migration / staged rollout / feature-flag batch switching or other processes that change the "do it right once, execute once" cadence

When you find yourself wanting to raise such a comment, drop it. A plan within the one-shot structure is either executable or **abandoned as a whole** (see "What leads to Abandon") — there is **no** middle option of "change the execution into step-by-step manual testing".

**Boundary — what is still legitimate (do not read it backwards)**:

- **Multi-purpose plan → Abandon + suggest splitting into several independent plans**: if a plan bundles several semantically unrelated things, the reviewer may judge Abandon and suggest splitting it into several single-purpose plans. What gets split is the top-level plan, not the task/wave flow of the same plan.
- What the "do it right once" section requires: artificially cutting down **the same feature**, or vaguely dumping known requirements onto "later" → still a finding. That judges whether the requirement is fully delivered, not the process structure.

**The plan's declared `目标` is locked once review-plan begins**: it is the commitment the developer set while writing the plan, belonging with `已归档的决策` to the established choices — plan review **must not overturn, expand, shrink, or redefine it**. The reviewer never writes a **goal-rewriting** finding such as "also do X while at it" or "this goal is too small / too big, change it to Y". **Only the developer can change the goal; anything where an agent wants to change the goal is dropped.** What remains legitimate is **checking the plan against the goal** (not changing the goal):

- The plan body deviates from / does not serve the declared goal, or under-delivers it (internal conflict / omission / "do it right once" → finding)
- The declared goal itself bundles several unrelated purposes → judge "multi-purpose plan" Abandon (judging "this goal is not single" reviews its cohesion as it stands, not rewriting it; see "Multi-purpose plan")

**The plan's `## 非目标` (optional, none by default) is locked just like the goal**: it is the explicit complement of the goal — directions the developer deliberately does not take. The reviewer **must not add / expand / redefine non-goals**, and even more **must not write a finding that pushes the plan toward any non-goal direction** ("also do X while at it" where X is exactly a non-goal) — that is entropy of the same kind as "expanding scope / changing the goal", dropped without exception. What remains legitimate is still checking as it stands: whether the plan body collides with its own declared non-goals (internal conflict → finding).

## The four outcomes of a review

| Outcome | Meaning | State written | Next action |
|------|------|-----------|----------|
| **Prerequisite failure** | The plan does not satisfy the review's preconditions (incomplete template structure, unresolved decision items, etc.) | Does not enter review; stays in plan authoring | The developer keeps refining the plan, then resubmits for review |
| **Abandon** | The plan was reviewed and has fundamental problems that local repair cannot salvage | `abandoned` | Abandon the current plan and regenerate from scratch |
| **Needs Refinement** | The plan was reviewed; direction is right, but there is ambiguity, omission or conflict | Stays `review-plan-in-progress` | Within the review iteration, `/address-review-comments` updates the plan and `/review-plan` runs again; only when the developer needs to **add a new open decision** does the developer add that decision and return the plan to authoring (see the template's 『需要决策的事项』) |
| **Executable** | The plan was reviewed; this round has no SUBSTANTIVE finding and all prior SUBSTANTIVE findings are reconciled as resolved (CONSISTENCY-only drift goes to the author's sync list and does not block; see the "Convergence" section) | `review-plan-complete` | The plan can be executed directly |

How **prerequisite failure** differs from the other three outcomes: prerequisite failure is a pre-check for entering review, not a review result. When the plan does not match the template structure, or still has unresolved decision items, the review flow does not start and the plan stays in authoring.

**The file contract carries no review checkboxes**: plan review must still judge whether the file list is complete, whether paths are real, and whether responsibilities and owners are sound; `review-plan-complete` is the only review-readiness signal by which the developer confirms the plan is executable, and also the state plan execution consumes. Do not build a separate per-file manual confirmation state.

**The verdict must be firm**: the chat verdict takes exactly one of "Executable / Needs Refinement / Abandon" (prerequisite failure is a pre-check, not a verdict). **As long as this round lists any SUBSTANTIVE finding, or a prior SUBSTANTIVE finding is not reconciled as resolved, the verdict is "Needs Refinement" (or "Abandon" when it reaches the Abandon level) — there is no "list SUBSTANTIVE problems and still judge Executable".** "Executable" only when this round's SUBSTANTIVE findings are empty and all prior SUBSTANTIVE findings are reconciled as root-cause resolved; CONSISTENCY-only drift goes to the author's "sync list" to align in place, **does not block Executable and does not trigger a new round** (the criteria for SUBSTANTIVE vs CONSISTENCY are in the "Convergence: SUBSTANTIVE vs CONSISTENCY" section). Do not write conditional verdicts such as "executable once X is fixed" — a conditional verdict pushes the judgment back to the reader, which is no verdict at all.

## Clean Architecture principles

The baseline is in the [architecture principles](architecture-principles.md), the single body shared by code review and plan review;
this section does not copy a second version. The plan's module design, responsibility division, API design, failure handling and complexity are all judged by that one.

Fixing a problem at the plan stage costs far less than at the code stage. One superfluous abstraction in a plan becomes, in code, interfaces, implementations and registration code scattered across several
files — a large change surface that is hard to roll back. So when reviewing a plan, be more alert to over-design and duplication than
when reviewing code: the items in the architecture principles that "require removal" (defensive fallback, superfluous abstraction, pre-built extension points,
do it right once) are all still just a few lines of text at the plan stage, and cutting them now costs almost nothing.

This guide adds only two items on the **plan review side**: the mechanical/semantic division of labor, and task-graph safety — the latter has no code-side counterpart.

Mechanical format rules are outside manual review — the ban on LOC/line-count estimates, the single-level token of the `大小` field, complete task structure fields, and `verify_task_graph.py`'s checks of the declared graph's ids, dependencies, file isolation and cycles are all intercepted deterministically by scripts. The reviewer does not re-copy mechanical findings; the file list and implementation tasks describe only responsibilities, boundaries, behavior and verification method.

### Task-graph safety: dependencies and isolation

Round 1 must have one narrow subagent (or the main reviewer serially when there is no subagent capability) review the semantic safety scripts cannot prove: every real producer/consumer or hard ordering has `blocked-by`, `consumes` matches the upstream `produces` contract, there are no unfounded false edges, ownership forms clear and stable exclusive boundaries, and each task can be independently and binarily accepted once its upstream completes.

`拥有文件` is the exclusive write/scheduling boundary between tasks and the outermost write ceiling, not a precise allowlist of expected affected files. It should prefer stable module or subsystem boundaries and may cover descendants the file contract does not name individually, to accommodate reasonable deviations such as new tests/helpers/re-export modules or lint-driven file splits. The reviewer must not demand file-level minimization merely because an owner is a directory, is broader than the expected file list, or includes unnamed descendants; whether the actually touched files still serve the goal is judged by the complete file contract, task goal/constraints, deviation report and task acceptance during execution. Report an isolation problem only when owners overlap, an expected touch point has no owner at all, a boundary spans unrelated areas and cannot form a clear area of responsibility, or ownership swallows tasks this plan has already explicitly split out to run safely in parallel.

New/modified/deleted files and concrete test files explicitly named by this plan must enter the file contract and be covered by one and only one owner containment; the file list's presentation exemptions for configuration, documentation, and import-only changes are not ownership/gate exemptions. `consumer_fallout.py` summarizes, task first, two classes of high-confidence chains — "direct reference + same-name test" and "modified source → direct production consumer → consumer's same-name test"; the complete low-confidence inventory lands only in the artifact and must not be injected wholesale into the reviewer's context. The reviewer must verify high-confidence unresolved items as covered by an owner/gate, or explicitly excluded in the plan's rationale; `omitted_unresolved_count` only triggers targeted evidence gathering in the relevant task bucket, not a full paste. A stable owner may be broader than the file list; an explicit path without an owner is not allowed, and the fallout inventory must not be mistaken for a precise affected-file allowlist.

The mechanical/semantic division of labor must hold: scripts catch declared cycles, dangling/duplicate ids, duplicate names, owner overlap, globs, missing consumes edges, and mechanically parseable explicit-path closure; the reviewer catches hidden semantic cycles caused by missing edges, false dependencies, wrong task boundaries, omitted fallout/SOT, and false independent acceptance. When any cycle is found, the only repair direction is to merge the tasks in the cycle and redraw the external edges; using ordering, dynamic edge removal or forced serialization to mask a cycle is forbidden.

Round 2+ is still a CLOSED WORLD: do not rerun the full task-graph fan-out. Only when a prior finding in this dimension is not settled, or this round's task-graph changed hunks directly modify ids/names, owners, edges, artifact contracts, task boundaries or acceptance gates, may a restricted narrow subagent be dispatched to review the changed hunks and their direct consequences.

A plan may say that a file needs its imports / exports corrected, but should not list every mechanical import / export statement one by one. For mechanical changes such as re-export (`pub use`) adjustments, crate/module migration, or path replacement, the plan only needs to state: the authoritative owner of the source symbol, the allowed/forbidden export boundary, the replacement pattern, the range of affected files, and the grep/`cargo check`/lint verification commands to run afterward. Line-by-line import instructions like "remove `use old::X;`, add `use new::X;`" are usually noise and should be left to the implementing agent to handle from the compiler and code context; only when an import/export change itself represents an architecture boundary or public API change does it need to be named separately.

## Guardrails

To the reviewer, the plan document is a set of **claims to verify**, not a source of facts. Adopting the plan's narrative and framing directly and then "checking" it means being anchored by the author's perspective — the value of independent review comes precisely from not sharing the author's derivation path.

1. **First understand the problem independently, then compare against the plan's solution** — When starting the review, first actually Read the documents listed under `参考资料` (applicable `AGENTS.md`, the repository's applicable architecture sources of truth, related plans) and the relevant source, independently build an understanding of the current state and constraints, and if needed derive for yourself "what shape a reasonable solution to this problem roughly has", and only then compare against the plan:
   - `当前状态分析` disagrees with the current state you read independently → finding
   - The shape of the plan's solution differs significantly from your independent derivation → trace which constraint supports the difference: if `已归档的决策` provides the basis, it stands (decisions are established choices, not judged); if no basis can be found anywhere in the document → finding
2. **The plan's paraphrase is not evidence** — When the plan says "the module SOT requires X", "method A accepts parameter X", "module B already has mechanism C", go back to the original/source to confirm every one; the basis of judgment can only be the original text, not the plan's paraphrase of it. Counts/limits/budgets, ordering and lifecycle guarantees, and universal assertions of the "only/always/never" kind are the highest-risk claims among them — verify each one, and for any that cannot be verified, give a finding with downgraded wording (e.g. "always" → "in the confirmed X scenario")
3. **Do not guess** — For facts you cannot confirm (whether a method exists, whether a parameter type is right, whether a dependency direction is legal), Read the source file to confirm before writing a conclusion
4. **Resolve conflicts by order of credibility** — source > module SOT / the repository's architecture sources of truth / official documentation > plan body. When a lower source conflicts with a higher one, judge by the higher one, and write the conflict itself as a finding
5. **Verify the verification commands themselves too** — For every verification command the plan proposes (test/grep/`cargo check`/lint, etc.), confirm the command really exists, runs in the right workspace/crate context, the filter actually routes to the target test suite, and it really checks the property it claims. A command that runs through with exit code 0 does not mean verification happened; selecting zero tests, bypassing the target harness, or declaring a coverage threshold without a corresponding coverage output/threshold check must all be reported as an invalid gate (typical case: a `cargo nextest run <filter>` whose filter matches no test still exits 0, so "the test passed" does not cover the behavior the plan claims)
6. **Lint belongs to the final combined tree** — a task gate should focus on task-specific unit/typecheck/build, and does not require mandatory scoped lint
   for every task. Every task gate must be marked `[TASK_LOCAL]` and must not use a complete platform build/full suite;
   the plan's global EXIT CHECK must use the canonical commands the template prescribes (`just lint` → `just test` → `just build`), fixed as final lint →
   full unit → optional full build/e2e; lint failures are routed to the owner for repair, and any repair that changes the tree restarts from final lint.
   Package, file or test-name filters may stay only in task gates and cannot pass themselves off as acceptance of the final combined tree.
6. **Reviewers are isolated from each other** — When several reviewers run in parallel, each reviewer reads only the round history of **its own lane**, and is forbidden to read other reviewers' review.md or any round artifacts (`temp/review-plan/<plan>/<other lane>/`). An independent perspective is a deliberate design: reading someone else's findings anchors reviewers on each other, and N reviewers degenerate into one. Findings from the lanes converge only on the plan author's side

## Calibrating the adversarial posture (--devils-advocate): skeptical, not contemptuous

Standard review already requires treating the plan's narrative as claims to verify (see Guardrails). The adversarial posture strengthens this one notch: distrust by default, actively falsify, and go after the failure modes the plan itself avoids. But "stronger skepticism" slides very easily into "contemptuous dismissal", which must be avoided.

**Skepticism = seeking truth more deeply, not denying more quickly.** It cuts symmetrically both ways: credulous neither of the plan's claims nor of the counter-explanation you use to overturn them. Like the UFO skeptic who blurts out "that's definitely a balloon" or "probably swamp gas" — grabbing the most convenient deflating explanation and calling it done — that is not skepticism but another kind of judging without review: replacing evidence with an equally unverified counter-claim. Credulity and contempt are two faces of the same disease, both substituting a convenient story for facts.

So under the adversarial posture:

- **First construct the strongest counterargument against every design choice**, but it only stands if it overturns the original approach on **evidence you actually read**; the counterargument must pass the same verification bar, and if it cannot overturn it, it cannot
- **Treat every paraphrase the plan makes of the codebase as unproven**, go back to the source to falsify or confirm it, and never "assume" on the plan's behalf (counts/limits, ordering and lifecycle, universal assertions of the "only/always/never" kind, "X capability already exists" assumptions, references to other documents/plans — the highest risk, each traced to a first-party source)
- **Specifically look for failure modes the plan never mentions, tests it avoids writing, simpler rival approaches it did not consider, hidden coupling and unstated assumptions**
- A counterargument that cannot be verified **cannot** become a finding — the original approach stands, or it is marked as an open item "pending evidence", rather than assuming it is wrong by default
- Still bound by Guardrails and "Finding admissibility / stop rule": adversarial means harder questioning and deeper verification, not more noise — no nit grinding, no style preferences, every finding first passes your own scrutiny. A clean "sound" conclusion must be marked "challenged" + what was tried

## Blocking vs automatic repair

When you find a problem, first ask one question: **"Will this problem make the implementing agent make an irreversible architecture mistake, or will it make a suboptimal implementation choice that tests can catch?"**

- **Architecture mistake or bad decision** → Needs Refinement or Abandon
- **Implementation detail** → repair it directly in the plan, then pass
- **Throughput first**: as long as no architecture risk is introduced, prefer automatic repair and pass, to avoid turning plan-review into a high-rejection-rate gate

> **Careful not to read this as "implementation-detail findings are allowed"**: "implementation detail → repair directly" in this section means that **an admissible macro finding already exists** and its repair happens to land at the implementation-detail level (such as a formula hidden behind an ellipsis), so the repair is written out clearly / given as a "possible fix" along the way. It does **not** allow raising a pure implementation detail as a finding by itself — that kind (naming, field splitting, execution-time ordering / state-machine enumeration, etc.) is dropped under "not admissible / plan sufficiency (over-spec boundary)". Criterion: **an implementation detail not backed by an admissible macro finding is not written; when a macro finding exists, its repair may go down to the implementation level**.

> **"Repair directly in the plan" depends on the writer mode** — the criteria for "repair vs block" are exactly the same in both modes; the only difference is who writes:
> - **Single-writer mode** (the same agent writes and reviews the plan, or the developer reviews it personally): the implementation-detail repair may be written directly into the plan, then pass.
> - **Formal review lane** (several reviewers in parallel, reviewer separate from author, i.e. the default form of `/review-plan`): the reviewer is **read-only** on the plan and gives the same repair as the **"possible fix" of an evidence-style finding** (as information, without an "adoption recommended" label and without urging adoption), and the plan author evaluates it and writes it in.

### Examples of repairing directly and passing

- An ellipsis hides a concrete formula or condition → fill in reasonable implementation detail
- The control-flow wording of a code snippet is ambiguous (e.g. unclear `return` semantics inside a closure) → change to explicit wording
- The error-handling semantics of a batch operation are unclear → pick a reasonable handling and write it into the plan
- A method's visibility description does not match actual usage but does not cross modules → correct the visibility

### Examples that must block

- The view layer directly references internal types of the underlying domain model, violating an archived module dependency constraint
- The business logic layer calls the state layer or view layer, inverting the dependency direction
- The plan relies on a public API that does not exist, and adding that API requires a cross-module change
- A new feature needs to modify another module's enum/trait definition, but the plan does not include that module

## Finding admissibility and the Ready threshold (stop rule)

"Macro-level review" must be an executable boundary; it cannot coexist with "exhaustive enumeration" — stacking the two pushes the reviewer into endlessly filling in specification, gradually turning the plan into a shadow of the implementation code. Below, **admissibility** constrains "what can enter the review", and the **stop rule** constrains "when to judge Executable".

**Admissible (is a finding) — only judgments that change the execution result enter the review:**

- Internal contradiction in the plan (body vs body, body vs `已归档的决策`)
- Mismatch with source / module SOT (plan claims inconsistent with the actual codebase)
- Wrong module ownership / dependency direction / architecture layering (including the 5 "Abandon" classes)
- Wrong lifecycle, resource release, or data-flow direction
- Implicit decision not archived (only require recording it, do not judge the choice itself)
- Test gate cannot verify the main goal (the test gate does not correspond to the capability the plan claims)
- An in-scope capability omitted from the execution list (files / tasks / tests)
- Plan body references the review process (round / issue numbers, reviewer opinions, `temp/review-plan` paths) → not self-contained; an implementing agent in a clean context cannot understand it
- Other macro problems that fit no existing category but are proven by first-party evidence to change the execution result; they must meet the extra admission of the "Other" dimension

**Not admissible (nitpicking, not written into the review):**

- Naming style of helpers / variables / types
- How the internal fields of a DTO / data structure are split, how private methods are organized
- Where mock / fixture files are placed
- Implementation details such as cache keys, constant values, log wording
- Unit test cases not listed "nicely" enough (covering the main goal and real risk boundaries is enough; exhaustive enumeration is not required)
- Demanding pre-built error handling / fallback / defensive branches for situations that cannot happen (violating an established invariant) or whose failure is app-fatal (no meaningful degradation path) — the former should fail fast (assert), the latter should be allowed to crash at a reasonable boundary; demanding local fallback is reverse over-defense (note: if the plan **already** designs it that way, the direction is reversed — require removal per the [architecture principles](architecture-principles.md) "boundary of defensive fallback")
- Any suggestion that changes the workflow structure — splitting the one-shot execution / inserting manual gates or manual testing between tasks or waves / moving e2e verification earlier / executing in batches because the plan is big / regrouping tasks by PR / staged migration — see the "Workflow structure is a fixed constant" section, dropped directly (the only exit for a structural problem is Abandon, not rearranging the process)
- **Exhaustive enumeration of execution-time implementation details**: all permutations of callbacks / events, latch fields, timeout control flow, exception combinations, the concrete organization of mocks — as long as the plan has fixed "target behavior + owning module + data/control-flow direction + invariants that must hold + verifiable success criteria", these are the implementing agent's judgment, and the plan is not required to prove them one by one (see "Plan sufficiency (over-spec boundary)"). Only when it can be proven that **the established architecture/protocol cannot be implemented** does some ordering / field / state-machine detail escalate to a macro finding
- Any detail the implementing agent can judge for itself that does not affect architecture direction

**Stop rule (the hard threshold for judging "Executable"):**

> "Executable" ⟺ this round's **SUBSTANTIVE** findings are empty, and all prior SUBSTANTIVE findings are reconciled as root-cause resolved. CONSISTENCY-only drift goes to the author's "sync list" to align in place, **does not block Executable**, and does not trigger a new round.

The stop rule constrains **what is not worth listing**, not "listed but still counted as Executable": details that do not change the implementing agent's architecture direction / module ownership / lifecycle / verification threshold / cross-platform contract, and that the implementing agent can decide for itself, are **not admissible** in the first place — **drop them (not written into the review)** per the nitpicking list above. **Convergence does not come from grinding fresh territory to zero**: Round 1 catches omissions in full; Round 2+ is closed incremental (only reconcile prior rounds + review this round's diff and its direct consequences); when a round's SUBSTANTIVE is empty and all prior substantive findings are reconciled, judge Executable — there is **no need** to go to unchanged regions to earn Executable with "covered new regions and still 0 findings". Continuing to run into unchanged text / unexplored source to force out ever finer contracts is review theater — the solution is **not to dig for it** (see the closed incremental rule in "Convergence: SUBSTANTIVE vs CONSISTENCY").

## Convergence: SUBSTANTIVE vs CONSISTENCY

Multi-round review diverges (reviewing a plan with a clear goal into a complete concurrency-protocol specification) for two root causes: **incremental rounds doing open-ended exploration**, and **any finding triggering a new round**. Convergence relies on the two hard rules below plus carrying source numbers across rounds, not on "grinding to zero".

**1. Closed incremental (Round 1 breadth, Round 2+ closed world).** Round 1 is a full breadth review (fan-out + fresh-eyes + all macro dimensions). **Round 2+ handles only three classes**, nothing else: (i) this lane's unsettled prior findings; (ii) the hunks changed by this round's diff; (iii) contradictions or regressions directly introduced/exposed by this round's diff. Incremental rounds are **forbidden** to: explore source not previously Read / scenarios not covered / tasks not traced, raise brand-new findings against unchanged text, or rerun the full fresh-eyes or complete dimension fan-out. The only exception: this round's diff changes the architecture direction and directly exposes a new macro problem.

**2. SUBSTANTIVE vs CONSISTENCY (only the former blocks; expressed by section, not by labels).** Problems fall into two classes, **and the class is expressed by which section it is written into, no longer by a per-item `分类` label**:
- **SUBSTANTIVE → written into "新问题与建议"**: changes what the implementing agent will build — architecture / module ownership / dependency direction / lifecycle contract / verification gate / omitted in-scope capability / conflict with source or archived decisions. **Only this blocks Executable.**
- **CONSISTENCY → written into "同步清单"**: the same intent stated out of sync (drift) across decisions / tasks / tests / file list, not changing what to build, only text not aligned. **Does not block**: the author aligns in place, without triggering a new re-review round.
- "This round's 『新问题与建议』 is empty" = converged → Executable. This rule directly ends the "change one place → drift → full re-review" spin.

**Abandon means "cannot be salvaged locally", not "too many rounds".** Abandon is used only for: a multi-purpose plan, a fundamentally wrong architecture direction, or a problem that cannot be repaired locally within the current diff. **A locally repairable SUBSTANTIVE finding — no matter which round — is "Needs Refinement", and does not escalate to Abandon because of the round count.** No hard cap on rounds: convergence is guaranteed by closed increments (rule 1) — each round only reconciles prior rounds + reviews this round's diff, and SUBSTANTIVE narrows naturally round by round.

**Cross-round / cross-lane dedup (no keys generated).** A problem carried over from a prior round keeps its **source number** (carries R<round>-<seq>) + location as its identity; one root cause is one entry throughout, only adding evidence, never reopened under a new title. Same-root findings across lanes are merged by the author by **location + semantics**; reviewers are not required to generate or maintain a stable key field. Several lanes raising the same root cause = one finding with higher confidence, not K units of work.

**Read the author's triage ledgers to suppress reopening decided items (cross-round + cross-lane).** The script reports `TRIAGE_LEDGER` (**all** /address-review-comments `triage.md` files of this branch, comma-separated old→new by mtime). Each triage run writes only the findings handled in that run and is not cumulative, so **all** ledgers must be read to suppress across rounds / lanes — reading only the newest misses already-decided items triaged earlier or by another lane; when the same problem (location + semantic root cause) appears in several ledgers, the **newer** decision wins. The ledger's three parts, applied / rejected / flagged, are all the information needed. Before any candidate finding enters "新问题与建议", compare it with the ledgers by **location + semantic root cause**: **rejected** = decided, do not raise again (not even reskinned; the only exception: this diff / new evidence conclusively overturns its factual premise → escalate a one-sentence "decision premise challenge", do not reopen it yourself); **applied** = only verify whether it was really fixed, and if so, no finding; **flagged** = pending developer decision, do not copy into a new finding. Suppressed candidates do not enter the output and do not block Executable. This is the mechanism that eliminates the residual divergence of "a new lane re-digging, every round, real problems the author already decided not to change".

## Plan sufficiency (over-spec boundary)

Once a plan has made these five items explicit, it is **sufficient to execute**, and the reviewer must not demand it spell out implementation-level details item by item:

- Target behavior (what to do)
- Owning module (where to do it)
- Data / control-flow direction
- Invariants that must hold
- Verifiable success criteria (test gates / acceptance criteria)

Beyond that, the **concrete permutations** of callbacks and events, latch fields, timeout control flow, exception combinations, and the organization of mocks are all **the implementing agent's judgment**, implementation details, not plan findings (same origin as the exhaustive enumeration of execution-time implementation details under "Not admissible"). Demanding the plan "prove it handles permutation X / edge timing Y" turns the plan into a shadow of the implementation code — unless it can be proven that **the established architecture / protocol cannot be implemented without specifying that detail**, do not raise it. This rule specifically blocks the over-design of "reviewing a plan with a clear goal into a complete concurrent state-machine specification".

## What leads to "Needs Refinement"

### 1. Ambiguity

A plan is a specification. When the implementing agent hits uncertainty it has only two choices: guess or stop. Both waste resources.

Common forms of ambiguity:

- **Ellipses in code snippets**: `...` or `// existing logic` is not itself a problem — the plan's code snippets illustrate intent, not verbatim templates. The question is: whether the omitted part involves an **architecture-level decision** (which layer the new logic goes into, which interface it calls, which data flow it takes) and the plan does not state it elsewhere. If the architecture intent is clearly expressed in other sections, the ellipsis only omits implementation details the agent can derive itself, and is fully acceptable
- **Vague implementation tasks**: a task may state only the outcome and not a method-level procedure, but if its `约束` / `验收闸门` do not pin down behavior, location boundary and binary result, it is still ambiguous. Do not use this to demand the task write a rote workflow.
- **Undefined components**: a task references a component, but `需要修改/添加的文件` has no structure definition or contract snippet for it
- **Unspecified interactions**: new component A calls component B, but B's interface is undefined; data is produced in module A, but how it is passed to module C is not stated

### 2. Omission

- **Earlier capabilities must land in files and tasks**: if the plan mentions a feature, behavior, state, component, service, API, test goal or user-visible capability in sections before `需要修改/添加的文件` and `实施步骤`, and does not explicitly mark it as "future work", "not done in this plan", "out of scope" or an equivalent, the reviewer must require it to appear in both:
  - `需要修改/添加的文件`: list the new/modified files carrying the capability and state each file's responsibility
  - `实施步骤`: taken on by one and only one task owner, with the data or control-flow level contract stated in the goal/constraints/acceptance gate
  Otherwise it is an under-specified omission. A plan of the form "the goal section said it, the implementing agent will fill it in" is not acceptable; in-scope capabilities raised earlier must be taken on by the execution list, to avoid delivering only scaffolding or placeholders at implementation time.
- A file is referenced in `实施步骤` but not listed in `需要修改/添加的文件`
- A concrete test file named in `测试计划` does not enter the file contract, or any explicit file/test path is not covered by a task owner containment
- Module structure, public API or a dependency edge changed, but the corresponding module SOT (architecture docs / `AGENTS.md`) does not enter the same task owner and acceptance gate; file-list exceptions do not exempt this closure
- A new public API has no corresponding test in `测试计划`
- The plan assumes a method or type exists, but it is neither listed in `参考资料` nor confirmed in a code snippet
- The plan lists a large number of mechanical import/export statements one by one; change it to state which files need import/export cleanup, the replacement pattern, boundary constraints and verification commands, keeping only entries that really affect the public API or an architecture boundary

#### Unit test coverage

Plan review cares about **behavior scenario coverage**, not line-coverage numbers. Coverage thresholds are checked by repository-level gates themselves (Bus has no per-file coverage gate; its gates are `just lint` / `just test`); the reviewer should not turn plan review into a textual check of "did it write 95%/100% coverage". The test plan in the plan must prove: the main usage paths, alternative paths, boundary conditions and failure paths of the new behavior all have explicit tests.

- Every new public method of the business logic layer or state layer must have at least one corresponding unit test case in the test plan
- Every new user-visible capability or core logic capability must list scenario tests by real use cases, not just one happy path. For example, hit detection should cover hitting the target, hitting blank space, out of bounds, overlap/multiple targets, coordinate transforms, alignment/offset, rotation/scaling and other scenarios that affect the result
- The test plan should cover every branch where an archived decision changes behavior: if Strict vs Nearest, scope-locked vs cross-layer multi-select, hard-cut signatures, carrier-aware write-back, etc. were chosen, there must be corresponding tests proving those decisions landed
- Boundary conditions should come from real risk first, not mechanical enumeration. Common high-value boundaries include: empty collections, a single element, several elements of the same kind, first/last positions, out-of-range input, id mismatch, duplicate ids, nonexistent targets, coordinates on the visual edge, pending edit / undo / redo midway through a state transition
- If some logic depends on geometry, time, ordering, serialization or cross-module state, the test plan must cover representative scenarios in those dimensions that change the result; it cannot test only the simplest coordinates, default time, default order or round-trip happy path
- For logic involving state transitions (A → B → C), every transition must have its own test, not just the final state
- Explicit conditional branches in code (such as `if is_locked { return; }`) must have a test case covering that branch
- Every case in the test plan must have a meaningful failure mode: it should go red when the implementation is wrong. A case that only observes the harness's own products (asserting that a just-configured mock was called, advancing a fake clock and asserting only the advance itself, a subscription test that only verifies the test's own subscription) is an invalid test; a test plan that relies on mocks, fixtures, fake clocks, subscriptions, lifecycle hooks or async effects must be able to explain that its assertions observe the target behavior rather than harness products
- If the plan adds an interface abstraction (trait) for mocking/dependency injection, the test plan must show its use
- If the plan adds unit tests for the state layer (app state / runtime / reducers, etc.), it must be explicit about:
  - What the test runner and harness are (this repository: `cargo nextest` via `just test`; unit tests in in-module `#[cfg(test)] mod tests`, integration tests under `tests/`)
  - Whether it reuses the patterns of existing stable test files (refer to existing `#[cfg(test)]` modules in `src/` and `tests/*.rs`)
  - Whether it uses real business-logic-layer fixtures or mocks/fakes (prefer real implementations over mocks)
  - When platform/OS dependencies are involved (PTY, terminal, sockets, filesystem), how they are replaced with fakes or temp directories so the tests run in CI
  - How timed cases (timers / timeouts / throttling and debouncing) are isolated, rather than casually rewriting the shared harness of an entire existing test file

### 3. Internal conflict

Conflicts appear most easily when the developer changes requirements or adds constraints during plan iteration:

- `已归档的决策` chose approach A, but code snippets are still written per approach B
- A file was removed from `需要修改/添加的文件`, but a task's `拥有文件` still references it
- A new requirement is reflected in some sections but other sections were not updated in sync
- A task's goal/constraints/acceptance gate contradicts a contract snippet in `需要修改/添加的文件`

### 4. Code inconsistent with the actual codebase

- Type names, method signatures, parameter types described in the plan do not match the actual source files
- The location where a code snippet is to be inserted does not exist in the source file (surrounding code has changed)
- A dependency the code snippet references (type, trait, function) does not exist in the codebase

### 5. Implicit decision not archived

`已归档的决策` itself is outside review scope (see "Archived decisions are outside review scope" at the top), but an **implicit decision** is in scope: an architecture choice silently made in the plan body (current state analysis / files to modify/add / implementation steps) yet absent from `已归档的决策` should be raised as an evidence finding (no disposition label) asking the developer to make it explicit and archive it.

When presenting an implicit-decision finding, only require **recording** it, do not judge the choice itself: point out where in the body the choice appears, what it effectively decides, and the suggested entry text to archive.

### 6. References to the review process; plan not self-contained

A plan is an independent specification for the implementing agent. At execution time it gets a **clean context** — it does not know how many rounds `/review-plan` ran, nor what "round 2 issue 3" refers to. So the plan **body** must not contain references to the review process:

- Wording such as "review round X pointed out…", "issue Y asked to change it to…", "per the reviewer's opinion…", "prior feedback…"
- Section titles / subheadings / parentheticals that **frame** a part as "a response to review", such as `## 可执行链路（回应审查）`, "response to reviewer", "adjustments for review comments" — such labels assume the reader knows a review happened, which an implementing agent in a clean context does not; titles should be named by the content itself ("可执行链路"), deleting process footnotes such as "（回应审查）"
- Any reference to round numbers, finding numbers, or `temp/review-plan/**` paths
- Writing the back-and-forth of the review conversation into the body as part of the plan

The conclusions of review iterations must be **digested into the plan itself**: decisions land in `已归档的决策`, approaches land in the corresponding sections, and it reads as a seamless, self-consistent specification from start to finish, not a series of "patch notes for a certain comment". When such wording is found → finding: require deleting the process references, or rewriting them as the plan's own statements (**what** the decision is and **why**, not "who asked for it in which round").

Mind the boundary: what is forbidden is the **plan document** (`docs/plans/*.md`) referencing the review process. The `前轮问题核销` ledger and `承 R<轮>-<序>` markers inside the review artifact review.md (`temp/review-plan/**`) are the review lane's internal trace mechanism and are used as usual — they serve reviewers' cross-round reconciliation and do not enter the plan handed to the implementing agent.

### 7. Other

This takes only macro problems that have first-party evidence, change the execution result, but cannot reasonably fit categories 1–6 above and do not reach the five Abandon conditions below.
The finding must explain why none of the existing categories applies; implementation details, style preferences, archived decisions, workflow constants and candidates excluded by the
stop rule cannot be stuffed into "Other". For example, if the plan explicitly arranges a second same-purpose method within the correct owner,
but it can converge locally by reusing or generalizing the existing method, it may go in this dimension and be judged "Needs Refinement"; if a new parallel mechanism has already formed, it is still judged Abandon under
"Reinventing the wheel" below.

"Other" only completes the macro classification for Needs Refinement; it does not create a new type of Abandon.

## What leads to "Abandon"

Abandon means the plan's foundation is wrong and cannot be salvaged by local patching. Any one of the following conditions means Abandon:

### 1. Multi-purpose plan

The plan tries to do several things at once. For example: first restructure a module, then add a new feature on top of the restructured base. That is two plans disguised as one. Restructuring and a new feature each have their own decision space, risk and verification criteria; mixing them means neither can be reviewed or executed cleanly.

The criterion is **whether the goal is cohesive, not the number of tasks**: against the `目标` declared at the start of the plan — do all the plan's tasks and file changes serve that one goal. Several coarse-grained tasks can together form one legitimate large one-shot plan; only when the top-level goal itself bundles several unrelated things is it judged multi-purpose. Do not misjudge because the plan is big or has many tasks.

### 2. Breaking existing module boundaries or architecture decisions

The plan breaks existing module boundaries or violates architecture decisions archived in earlier plans in order to implement a new feature. Module responsibilities and boundaries should stay relatively stable unless a dedicated restructuring plan changes them. New features must be implemented within the existing architecture constraints, not by bypassing or breaking them for convenience.

For example: a state-layer module has had its dependency on the rendering layer removed by a dedicated restructuring plan. If a new feature plan reintroduces a "state layer → rendering layer" dependency, the plan should be abandoned.

### 3. Reinventing the wheel

An existing service or mechanism in the codebase can meet the need, but the plan creates a new parallel mechanism doing the same thing. This leads to several ways of solving the same problem in the codebase, raising maintenance cost and creating confusion.

For example: `src/app/state.rs` already owns the application state; if the plan creates an independent parallel state store to manage part of that state (instead of reusing the existing app state), the plan should be abandoned.

Boundary: a local duplicate method or behavior path within the same correct owner, if it can converge locally by reusing, upgrading or generalizing the existing capability,
goes in "Needs Refinement → Other"; only when the plan establishes an independent service / store / owner / public mechanism to carry an existing purpose,
forming a parallel source of truth that local change cannot eliminate, is it the Abandon-level "reinventing the wheel" of this section.

### 4. Introducing a cyclic dependency

The plan's module design causes an A → B → A cyclic dependency. This is not a problem fixed by moving a few methods — it shows the whole responsibility decomposition is wrong.

### 5. Business logic in the wrong layer

The plan puts business logic in the wrong architecture layer. We follow Clean Architecture: the business logic layer carries core logic, the state layer keeps only state and workflow orchestration as far as possible, and the view layer only renders and forwards events. It is normal for the view layer/state layer to control UI display based on state (such as showing/hiding a button), but if the plan stuffs logic that belongs in the business logic layer or another dedicated module into the view layer or state layer, the architecture design has a fundamental problem.

For example: layout geometry calculation belongs in `src/layout.rs`. If the plan writes pane split or hit-test logic directly inside a `src/ui/` widget, the plan should be abandoned.

Another common counterexample: a feature's core rules, batch data transformations, geometry calculations, and reusable query logic could have been exposed as a general business-logic-layer API to several callers, but the plan writes them as part of some state-layer/view-layer private method just because the current button happens to need them. Even if such a plan runs in the short term, it keeps the state layer bloating and hinders future reuse.

> Note (**keep the boundary tight; do not treat this as a back door to reopen archived decisions**): distinguish two cases —
> - **The plan body's implementation introduces new** structural damage the archived decision did not require (e.g. the decision only chose "put it in module X", and the body's concrete implementation additionally causes a cyclic dependency / layer violation) → the reviewer judges Abandon for **this implementation design** under items 1–5, legitimately: what is judged is the part of the implementation the decision did not require, not the decision itself.
> - **The damage is the direction the archived decision itself chose** (the decision explicitly set it that way) → this is an **established choice, outside review scope**, and must not be reopened under the guise of "I am reviewing the plan body's design". If the reviewer is convinced the decision's **factual premise** no longer holds (it would cause a cyclic dependency / layer violation or another hard defect), escalate it to the developer as a "decision premise challenge" (they may reopen the decision), **rather than** the reviewer judging Abandon on its own.
>
> In one sentence: what can be judged Abandon is "new damage the decision did not require and the implementation created itself"; "the direction the decision chose itself" can only be escalated, not re-reviewed under a borrowed shell.

## Finding output specification (evidence-style)

**Review output is an evidence report (CSI report), not a verdict and not a persuasive essay.** Each finding states facts and lets the developer make the disposition judgment from the evidence. **Disposition labels on individual findings are forbidden** — `阻断` / `建议采纳` / `废弃` / `需要完善` are never written on a finding. "Adoption recommended" literally means "I recommend you adopt this", pushing the developer toward acceptance before they judge; "blocking" is a verdict made on their behalf. The reviewer's judgment of severity happens only internally, used solely to summarize into the final **one** verdict among the "four outcomes".

Each finding presents:

1. **Location** — which section / exact location in the plan
2. **Observation** — what it is, stating facts without slipping in disposition advice
3. **Evidence** — the plan text / source / cross-references proving the observation, listed one by one as path:line
4. **Impact** — what will happen when the agent executes if it is not resolved
5. **Possible fix / options** (optional, provided as information, without urging adoption) — a concrete repair for the developer's reference

After the developer makes a choice:
1. Record the choice in the plan's `已归档的决策`
2. Update all affected parts of the plan (code snippets, implementation steps, test plan, etc.) to keep the whole document consistent
3. Recheck consistency to confirm no new contradiction after the update
