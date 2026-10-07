# Code Review Guide

The review agent receives `git diff <base>...HEAD` with `docs/plans/**` excluded, plus an optional linked plan document, and judges whether the code change can be merged into master. `<base>` is an optional input parameter (a commit hash); when unspecified it defaults to `origin/master`.

It is not lint, not code polish, and not plan review. Code review focuses on the implementation's architecture boundaries, correctness, security, maintainability, and the long-term health of the codebase.
Code review is a CI quality gate covering the full range from **macro (architecture/module) to micro (type/function)**.

This guide is language-agnostic: the principles apply to Rust, Python, shell, or any other target language. Where concrete syntax is involved, follow the target language's conventions, and write examples in the target language closest to the current code. Module boundaries and dependency direction should be confirmed against the code, its callers and tests, and the applicable architecture sources of truth in this repository.

## Review context: existing behavior is a verified baseline

Code review's position in the workflow determines its default posture. A diff that reaches the reviewer has usually **already been manually tested** (the author has verified by hand that the feature works), and its architecture and design have **already passed plan review**. This is the same kind of settled premise as plan review's "archived decisions are out of review scope":

- **Existing runtime behavior is a verified baseline, not a suspect for you to falsify.** Passing manual testing is positive evidence that the behavior is correct; passing plan review is positive evidence that its architecture is sound. The reviewer's job is **not** to assume everything is broken and then prove diligence by listing problems.
- **"No findings" is a legitimate and common conclusion.** For a promising PR that has been manually tested and plan-reviewed, the most common healthy outcome is: no behavior change needed, update docs if necessary. Do not manufacture findings to "look like you reviewed something" — manufactured findings are exactly the source of regressions entering the code.
- **The burden of proof for a finding that changes existing behavior lies with the reviewer.** Any correctness (dimension 2) / Bug (dimension 7) finding that would **change existing runtime behavior** must give **positive evidence** that the behavior is actually wrong: a concrete input that can reach the path + the wrong output it produces. "Null isn't handled here" or "this branch wasn't considered" without a reachable input that triggers it is no basis for changing behavior — it conflicts with the positive evidence of manual testing and ranks lower in the conflict ordering (see guardrail 6). **Proof should not stop on paper: writing the suspicion as a test and running it is the reviewer's default action**, and the resulting red light is the strongest positive evidence (see "Verification boundary").
- **Try to prove it first; harden only when it cannot be proven.** When you suspect a risk somewhere, the first action is **to write it as a test and run it**: red = proven, report it as a finding in the corresponding dimension; green = the current behavior is correct for that input, **withdraw the suspicion**, and add a doc note if needed. Do not propose "change the behavior here" without a red run, and do not cling to the original suspicion after a green run. Behavior that can only be verified in a real running app is reported marked "unproven" per "Verification boundary". Changing a manually verified behavior that you cannot falsify is what causes regressions.
- **Assist development; do not take over QA.** Code review covers statically decidable problems (architecture, correctness, boundaries, resources). A Bug finding should be a failure that occurs naturally in normal use; **deliberately orchestrated, order-dependent multi-step operation combinations constructed to "find something" should be suppressed** — leave such timing combinations to the QA process; the reviewer must not invent them (criteria in dimension 7).
- **Each PR's single purpose is the locked goal.** With a linked plan, the goal is taken verbatim from that plan's `## 目标`; without a plan, the first `review-pr` run must ask the developer for a one-sentence single goal and persist it per branch, reused by later lanes / rounds; **it must not be inferred from the diff, commits, or PR description**. The goal is a settled commitment that **only the developer can change** (plan-review-guide: "the workflow structure is a settled constant: the goal is locked"). The reviewer checks whether the code correctly serves that goal, but does not bear responsibility for item-by-item acceptance of plan completeness, and must not rewrite or expand the goal through a finding. Dimension 6 uses the locked goal as its only scope baseline and writes each out-of-goal change slice explicitly as a review comment: `XS` / `S` go into the non-blocking sync list, `M` and above go into new issues with a recommendation to split via `split-pr`; this scope comment does not authorize the reviewer to deeply review the second purpose. The linked plan's `## 非目标` are directions the developer deliberately excluded; like the goal, non-goals can only be changed by the developer, and the reviewer must not add, expand, or redefine them.
- **The linked plan's `已归档的决策` are likewise locked; review-pr must not overturn them.** Archived decisions are settled choices the developer already made when writing the plan (consistent with plan review's "archived decisions are out of review scope"). The reviewer **must not produce findings that overturn / reopen archived decisions** (e.g. "should not use option A, should use B" where A is exactly an archived decision) — the technology choice and architecture direction were settled by that decision; code review can only check whether the code **is consistent with the archived decisions** (code that violates an archived decision = a legitimate finding: the implementation drifted from the settled direction), not re-review the direction itself. The only exception: when you are certain an archived decision's **factual premise** has been overturned by this diff or new source evidence, hand it to the developer in one sentence as a "decision premise challenge" (they may reopen the decision); do **not** change the code toward the new direction yourself or judge Abandon on that basis.

## Review scope

All changes under `docs/plans/**` are excluded from code review scope: they do not enter the diff snapshot / delta, the touched-file
set, findings, the sync list, or the PR single-purpose judgment. The linked plan may still be read explicitly via `review-pr --plan`, but only as
input for the locked goal, non-goals, and archived decisions; the reviewer does not review, comment on, or request corrections to the plan file
itself. If there are no other committed changes after excluding `docs/plans/**`, there is no code to review.

Review is premised on "goal-relevance admission" and is not limited to changed lines in the diff. Reading touched files, direct callers, and contract dependencies serves to judge the owner, invariants, and impact of the goal-related implementation; it does not thereby grant general audit authority over the whole file or module.

For example:
- the diff adds a new method to a service, but that service already has 30 public methods with responsibilities spanning three domains → suggest splitting
- the diff modifies state management in some store/state layer, but it carries computation that belongs to the business logic layer → suggest pushing it down
- the diff adds a parallel state manager, but an existing store already has a similar mechanism → reinventing the wheel

Findings outside diff lines must still satisfy goal-relevance admission and this round's coverage boundary; they cannot be raised as needs-refinement suggestions merely because they sit inside a touched module.

## Goal-relevance admission

**Before** chasing a candidate, writing a probe, delegating an investigation, or outputting a finding or sync item, first establish from first-party context:
which locked-goal requirement it relates to, or which dependency / invariant that feature requires; and what concrete consequence not handling it would have for that requirement.
Note both briefly in the candidate's "影响" (impact). The same applies to architecture, maintainability, logging, and wording sync; structural problems are not required to fabricate a runtime reproduction.

**Goal relevance intersects with the round boundary**: diff coverage, the delta, unclosed prior-round findings, prior APPLYs, and incomplete coverage
cannot expand the locked goal. A file having been changed, a small fix, a red test, or multi-lane agreement each provide only their own facts and cannot supply goal authorization.
Paths are not the admission rule: a shared helper not named by the goal may still be chased if it genuinely breaks a contract the feature requires; an unrelated defect in the same file may not.

Dimension 6 is the only scope-visibility exception to goal-relevance admission: identify each out-of-goal change slice against the locked goal, verifying only its
paths / hunks, independent purpose, cumulative implementation and validation cost, and blast radius, without turning it into a correctness, architecture, or quality review of the second purpose's internals.
Changes for the same incidental purpose across commits / deltas are measured together and cannot be split into multiple `XS` to evade sizing. `XS` / `S` slices are allowed to stay with the main PR under the
Good Samaritan rule, written into the sync list as CONSISTENCY drift; `M` and larger slices are written into new issues as
SUBSTANTIVE findings, whose possible fix explicitly recommends the developer invoke `split-pr`. An incidental fix having blocked the main goal
is strong evidence supporting Good Samaritan, but not a necessary condition, and it does not change the size thresholds. None of these comments rewrites the locked goal,
nor grants the reviewer permission to keep chasing or to demand fixes to out-of-goal code.

Apart from dimension 6 scope comments, observations without goal relevance may at most be noted briefly in the file-only "本轮探索区域" (this round's exploration areas), and must not enter any other
"新问题与建议" (new issues and suggestions) or "同步清单" (sync list),
must not affect Ready, trigger author fixes, or start a new round. When a review tool blocks the flow from executing, report the execution limitation separately;
the tool is an instrument used in this round and does not prove it is a product dependency of the reviewed feature. When the review is incomplete, do not disguise blocked execution as Ready.

An incremental round first rechecks goal relevance of unclosed items: apart from dimension 6, your own out-of-scope findings are recorded as `withdrawn`; accepting the author's
scope rejection is recorded as `rejected`. Prior-round dimension 6 issues are re-sized by the current cumulative slice: split out / deleted → `satisfactory`;
shrunk to `XS` / `S` → close the prior SUBSTANTIVE item and record the current drift in the sync list; only if still `M` or above is it carried forward and re-raised.
A prior APPLY does not modify the goal, nor does it make another failing input in an unrelated subsystem eligible for reopening; only new evidence of actual goal impact or
an explicit scope change by the developer may bring it back into consideration, and it must still satisfy the round boundary. After closing out-of-scope items, recompute the conclusion from the remaining qualifying items and real coverage;
"reviewed only the incidental fix and found nothing" cannot count as completed goal-related coverage.

## Three verdicts

| Verdict | Meaning |
|------|------|
| **Ready** | The code can be merged |
| **Needs Refinement** | The code can be merged, with improvement suggestions |
| **Abandon** | The code should not be merged |

Only abandon blocks the PR. needs-refinement is a non-blocking suggestion.

**Ready's hard threshold**: this round's due goal-related coverage is complete, this round has no qualifying SUBSTANTIVE finding blocking Ready, and all prior-round items are legitimately closed per "Goal-relevance admission". `satisfactory` means the root cause is resolved; evidence-backed `rejected` / `withdrawn` close items that should not be pursued further, without claiming their code is fixed. As long as a qualifying SUBSTANTIVE finding that should be handled remains, the verdict is at least **Needs Refinement** (**Abandon** if it reaches abandon level). Behavior suspicions marked "unproven" (see "Verification boundary") **do not block Ready and do not trigger a new round**; other dimensions are judged by first-party source evidence and need no "proven" mark. In-goal wording drift and dimension 6 `XS` / `S` out-of-goal slices go in the "同步清单" (sync list), **do not block Ready, and do not trigger a new round**. Nitpicks are simply omitted; out-of-scope items other than dimension 6 are excluded by the admission rule. Having no findings cannot substitute for completed coverage.

## Convergence: SUBSTANTIVE vs CONSISTENCY

Multi-round code review also diverges (every round re-digs already-covered code, and any finding triggers a new round). The following rules constrain only work that passes "Goal-relevance admission" and do not expand the goal; convergence relies on two hard rules + carrying source numbers across rounds, aligned with plan review:

**1. Closed increment (Round 1 full, Round 2+ closed world).** Round 1 reviews the full diff. **Round 2+ only handles**: (i) unresolved prior-round findings; (ii) hunks of this round's delta; (iii) contradictions/regressions directly introduced by the delta. The only exception is completing coverage Round 1 did not finish (the prior ledger explicitly records it as "not yet reviewed", not "reviewed but want to dig more"). Incremental rounds **must not** re-review covered unchanged code to invent new findings, or treat exhaustive enumeration of runtime timing/exception combinations as Bugs (see "Assist development; do not take over QA").

**2. SUBSTANTIVE vs CONSISTENCY (only the former blocks; expressed by partition, not labels).** The category is expressed by which section it is written into; no per-item `分类` label:
- **SUBSTANTIVE → written into "新问题与建议" (new issues and suggestions)**: changes to code correctness / architecture / module boundaries / lifecycle contracts / security / existing behavior that would regress, plus dimension 6 out-of-goal slices of `M` and above. Only SUBSTANTIVE blocks Ready; among them, behavior suspicions in dimensions 2 / 3 / 7 count as blocking only when proven, while other dimensions stand on first-party source evidence. Suspicions marked "unproven" in the same section are listed as usual but do not count as blocking.
- **CONSISTENCY → written into "同步清单" (sync list)**: usually naming/comment/doc drift and pure style alignment; dimension 6 `XS` / `S` out-of-goal code slices are an explicit exception, because they only record that the PR scope disagrees with the locked goal and do not negate the Good Samaritan fix. None of them blocks or triggers a new round.
- Empty findings only mean there are no new problems; Ready must still meet the full hard threshold in "Three verdicts".

**Abandon means "not locally salvageable", not "too many rounds".** Abandon is used only for a fundamentally wrong architecture direction or a problem that cannot be locally fixed within the current diff; dimension 6 scope drift by itself never yields Abandon. **A locally fixable SUBSTANTIVE — in whatever round — is Needs Refinement and is not escalated to Abandon because of round count.** There is no hard cap on rounds: convergence is guaranteed by the closed increment (rule 1) — each round only reconciles the prior round + reviews this round's delta, so SUBSTANTIVE narrows naturally over rounds.

**Cross-round / cross-lane dedup (no keys generated).** An issue carried from a prior round keeps its **source number** (carried from R<round>-<seq>) + path:line as identity; one root cause stays one entry throughout, only adding evidence, never reopened under a new title. Same-root issues across lanes are merged by the author judging **path:line + semantics**; reviewers are not required to generate or maintain a stable key field. Multiple lanes raising the same root cause = one finding with higher confidence, not K units of work.

**Read the author's triage ledgers and suppress reopening of decided items (cross-round + cross-lane).** The script reports `TRIAGE_LEDGER` (**all** /address-review-comments `triage.md` files on this branch, comma-separated old → new by mtime). Each triage run writes only the findings handled in that run and is not cumulative, so **all** ledgers must be read for cross-round / cross-lane suppression — reading only the newest misses decided items triaged earlier or by another lane; when the same issue (location + semantic root cause) appears in multiple ledgers, the **newer** decision wins. The ledger's three sections, applied / rejected / flagged, are all the information needed. Before any candidate finding enters "新问题与建议", compare it with the ledgers by **location + semantic root cause**: **rejected** = decided, do not raise again (not even reskinned; the only exception: this diff / new evidence conclusively overturns its factual premise → hand up a one-sentence "decision premise challenge", do not reopen it yourself); **applied** = only verify it is actually fixed, and if fixed, no finding; **flagged** = pending developer decision, do not copy it into a new finding. Suppressed candidates do not enter the output and do not block Ready. This is the mechanism that eliminates the residual divergence of "new lanes re-digging, every round, real issues the author has decided not to change".

## Clean Architecture principles

The criteria are in [Architecture principles](architecture-principles.md), the single body of text shared by code review and plan review;
this section does not copy a second version. When judging whether code violates layering, dependency direction, failure handling, complexity, or change discipline, read that one.

This guide only adds the **code-review-side routing rules**, to avoid reporting the same root cause twice:

- The full checkpoints and severity for over-engineering, excessive fallback, and branch proliferation all belong to [dimension 8: Over-engineering and branch proliferation](#8-over-engineering-and-branch-proliferation). The architecture dimension only judges owner, layering, boundaries, and dependency direction, and does not report the same root cause as dimension 8.
- Concrete DRY findings likewise belong to dimension 8.
- The architecture principles that "require removal" (defensive fallback, superfluous abstraction, pre-built extension points, deprecation transition periods) land in code review as dimension 8's "remove the superfluous path" direction, and never produce an "add a fallback" finding.

## Review dimensions

> Dead code is not in this list: unused parameters/functions/types, empty functions/empty types, variables assigned but never read, commented-out old code,
> unreachable branches, and the pair "dead implementation + zombie tests that only test it" are all handled by the `delete-dead-code` skill before the gates.
> It mechanically scopes by the modules the PR touches and fans out per module; neither its coverage nor its evidence strength can be matched by a single review;
> reporting a few more during review would only duplicate or conflict with its conclusions.
>
> Item-by-item plan completion is accepted by the execution and gate flow; technology choices are settled by `review-plan`, and `review-pr` does not re-examine the question.
> `TODO` / `FIXME` / `XXX` are intercepted mechanically by project lint and are not a manual review dimension.

### 1. Architecture and design

This dimension only reviews owner, layering, module boundaries, and dependency direction. Technology choice belongs to plan review; duplicate methods, duplicate behavior paths, and unnecessary complexity within the same owner belong to dimension 8, and the same root cause must not be reported twice. Only when a duplicate mechanism also breaks an owner or module boundary is that boundary root cause reported under this dimension.

Checkpoints:
- whether new units sit in the correct architecture layer (business logic / state / view / infrastructure)
- whether module boundaries are broken (against actual imports and call sites, and the repository's architecture sources of truth)
- **whether visibility is minimized** — whether new symbols are narrowed to the smallest scope that satisfies current callers (see [Architecture principles "Minimal public API (visibility)"](architecture-principles.md#minimal-public-api-visibility))
- whether dependency direction is correct

Severity:
- business logic placed in the view/state layer and not simply movable → **abandon**
- circular dependency → **abandon**
- breaking an existing module boundary → **abandon**
- a public symbol that can be narrowed to module-internal → **needs-refinement**
- a trend of module responsibility bloat → **needs-refinement**

### 2. Correctness

Checkpoints:
- empty-value handling: `None` / `Option` / `Result` / empty collections
- boundary conditions: off-by-one, empty input, oversized input, concurrency
- state management: whether transitions are complete, whether illegal states exist
- resource management: whether streams, connections, subscriptions, child processes, PTYs, and file handles are released correctly
- error handling: whether errors are silently swallowed
- concurrency safety: race conditions, async/thread safety

A correctness finding that would **change existing runtime behavior** must follow "Review context: existing behavior is a verified baseline" — give a reachable input + wrong output proving the current behavior is actually wrong. **The preferred form of proof is writing a test and running it red** (see "Verification boundary"), recording the test name + test file relative path + the observed failure as evidence; if you cannot get a red run, or it only turns red by changing the implementation, withdraw the suspicion and do not make it a behavior-changing finding; if it can only be verified in a real running app, mark it `未证明` per "Verification boundary".

Severity:
- certainly causes a crash or data loss → **abandon**
- a race condition that may cause intermittent crashes → **abandon**
- incomplete boundary conditions that do not affect the main flow → **needs-refinement**
- error handling could be more precise → **needs-refinement**

### 3. Security

Checkpoints:
- hard-coded keys, tokens, passwords
- SQL/command injection, XSS
- sensitive information stored in plaintext
- unnecessary permission requests
- sensitive information in log output

Severity:
- any security vulnerability → **abandon**

### 4. Log coverage

Logs exist to reconstruct user operations, key state changes, cross-boundary results, and failure causes, not to record function control flow. Rust code uses
`tracing`, following the conventions of the surrounding module.

Principles:
- Each business event is recorded once by the single responsible layer closest to its owner; layers along a call chain must not record the same fact repeatedly.
- Log density is determined by the information value of the event, not by function length, entries, exits, returns, or branch count.
- If control flow can only be understood through a mass of play-by-play logs, simplify the control flow first rather than adding logs.
- A recoverable external failure must keep its cause and necessary context at its responsibility boundary; an internal invariant violation must fail fast and must not be logged and then continued.
- Pure computation, simple reads, and pure presentation produce no routine logs.

Severity:
- a key business event or recoverable failure lacks diagnosable evidence → **needs-refinement**
- the same event recorded repeatedly by multiple layers, or logs degenerated into a function play-by-play → **needs-refinement**
- an impossible state logged as an error and then execution continues, instead of failing fast → **needs-refinement**
- the original `cause` / source error discarded when raising a new error → **needs-refinement**

### 5. Naming and comment quality

Function names, type names, and field names are documentation in themselves. Names should be descriptive enough that a reader knows what something is, what it does, what it accepts, and what it returns without reading a comment. Any comment that only describes "what this is", "what it does", "what the parameters are", or "what it returns" — when the name already expresses this — is redundant and should be deleted. This rule applies to functions, types, fields, and the parameter/return sections of doc comments / docstrings.

**Redundant comment examples:**

```rust
// Checks whether the element is selected
fn is_element_selected(element_id: &str) -> bool
```
```rust
// Set of currently selected element IDs
selected_element_ids: HashSet<String>
```

**There are only three legitimate reasons for a comment to exist:**
1. **Design decision** — explains why this approach was chosen over a more obvious one; the comment must be self-explanatory and cannot merely cite a decision number in a plan
2. **Domain knowledge** — business rules or industry conventions that a reader without engineering context cannot infer from the code
3. **Pitfalls and warnings** — non-obvious side effects, timing dependencies, platform behavior differences; things you only know after getting burned

Code comments must not depend on plan-file context. Plans may be archived, deleted, or rewritten after execution; code lives on. Comments like `// Decision 11 / 14: ...`, `// per plan Q2=B ...`, `// Phase 3 ...` shift the cost of understanding onto external documents; review should require them to be rewritten as a complete reason.

```rust
// The kernel may deliver a PTY read of zero bytes before the child exits; treating it
// as EOF here drops the child's final output. Only `waitpid` reporting exit is the real
// end of the stream, so keep draining until then.     ← pitfalls and warnings
```

Criterion: after reading the comment, look at the name again; if the comment provides no information beyond the name, it is redundant.

Repository convention: all new comments are written in **English**.

**Spelling must be correct**: English words in function names, parameter names, type names, and variable names must be spelled correctly. Common mistakes such as `recieve` → `receive`, `seperate` → `separate`, `occured` → `occurred`, `lenght` → `length`.

Severity:
- redundant comments (including parameter/return doc sections) → **needs-refinement**
- comments that only cite a plan decision number, issue number, or option code name without explaining the reason next to the code → **needs-refinement**
- spelling mistakes → **needs-refinement**

### 6. PR single purpose

The PR's single purpose is defined by the locked goal and is not re-derived from the diff, commits, or PR description. First split the committed diff, with
`docs/plans/**` excluded, into two kinds: changes that serve the locked goal and its necessary dependencies / invariants, and out-of-goal change slices that can be
independently reviewed, merged, and reverted. Tests, docs, necessary refactors, or direct invariant fixes are not a second purpose as long as they genuinely serve the
locked goal; conversely, code being useful, tests passing, or living in the same file cannot turn an independent purpose into an in-goal one.

Every out-of-goal slice must become a review comment, but dimension 6 reviews only its scope identity and size, not the internal quality of the second purpose:

- Assess understanding, implementation, review, and validation cost plus risk / blast radius using the plan template's unified size scale; LOC and file count are only signals.
- The same incidental purpose is assessed cumulatively across commits, files, and later deltas; it cannot be artificially split into several small slices.
- `XS` / `S` → write into the "同步清单" (sync list), one line stating path:line, the out-of-goal purpose, the size tier, and evidence of its disagreement with the locked goal.
  This is a small fix Good Samaritan allows to stay with the main task; especially applicable when it once blocked the main goal,
  but "once blocked" is not a necessary condition. The comment only records scope drift; it does not require deletion or splitting and does not block Ready.
- `M` and above → write into "新问题与建议" (new issues and suggestions), with internal severity **needs-refinement**; the "可能的修复" (possible fix) recommends the developer
  invoke `split-pr` to split that purpose into an independent PR. The reviewer does not invoke the skill, move commits, or rewrite the locked goal.

Dimension 6 by itself **never yields abandon**. Even a large out-of-goal slice is first raised as a review issue requesting a split; only when the locked goal
has another fundamental architecture error or a problem that cannot be locally salvaged does the corresponding dimension independently yield Abandon.

### 7. Bug (unanticipated edge cases)

A bug is not a correctness problem (correctness is covered by unit tests), but **an unhandled, unanticipated edge case that causes the system to partially or completely fail**. Such problems usually occur at the user interaction layer: the code logic itself is right on the normal path, but under specific conditions the user ends up in an unrecoverable state.

Typical patterns:
- **Dead-end navigation** — a key or action moves to a view, but that view has no way back, no escape key, no exit mechanism of any kind; the user is trapped
- **Undismissable popup/modal** — a dialog or overlay appears, but there is no close action, clicking outside does nothing, Escape does nothing
- **Deadlock state** — the state machine reaches a state with no outgoing edges, and the user cannot trigger any action to recover to the normal flow
- **Irreversible destructive operation** — the user triggers a delete/reset with no confirmation prompt and no undo mechanism
- **Input loss** — the user edited content, and a resize, detach/reattach, or accidental navigation loses all input with no way to recover it

Inspection method:
- do a mental walkthrough of navigation, popups, and state transitions added or modified in the diff
- ask: "What can the user do in this state? If the answer is 'nothing', it's a bug"
- check that every new view/popup has an exit path
- check that every state of the state machine has at least one outgoing edge

A bug is "an unanticipated edge case causing failure", not a supposition. The mental walkthrough must land on a **concrete operation sequence that reaches the state** — if the walkthrough cannot produce a real path there, or the test written for it does not run red, it is a suspicion that cannot be proven under "Review context: existing behavior is a verified baseline": withdraw it if green; if it can only be verified in a real running app, mark it `未证明` per "Verification boundary"; neither becomes a behavior-changing Bug finding.

**Do not chain "combos" to "find something".** Code review assists development; it does not take over QA. A Bug finding should be a failure that occurs naturally in normal use; **deliberately orchestrated, order-dependent multi-step operation combinations built to force a failure** (such as "close A → drag to the edge in state B → reopen A → undo") should be suppressed — leave such deliberate multi-step timing combinations to the QA process; the reviewer must not invent them. The more deliberately orchestrated an operation chain is and the more it depends on a specific trigger order, the more likely it is the reviewer's imagination rather than something users will actually hit.

Reporting requirements (TDD principle):

When a bug is found, the review agent must provide all three of the following:
1. **Reproduction test** — actually write the test and run it narrowly to red (see "Verification boundary"), recording the test name + test file relative path + the observed failing assertion in the evidence. The test file stays in the working tree for the author. If you cannot get a red run, it is not a Bug finding
2. **Fix suggestion** — a concrete fix (which file to change, how, and why this way)
3. **Regression test** — how the test should turn green after the fix (the assertions expected to pass)

If the bug involves a user interaction flow (navigation, popups, state transitions), suggest unit tests covering the state machine/logic layer to verify the fix.

Severity:
- the user is trapped and must force-quit the app → **abandon**
- a destructive operation with no confirmation and no way to reverse it → **abandon**
- an edge case that degrades functionality but has an alternative path → **needs-refinement**

### 8. Over-engineering and branch proliferation

The goal is one strong, stable, unique happy path. The reviewer must stay skeptical of the current implementation and require every piece of complexity to prove that it serves the current locked goal, instead of treating complexity as robustness.

Principles:
- Choose the simplest form that correctly satisfies the current contract; the burden of proof for complexity lies with the implementer.
- One behavior has one owner, one source of truth, and one main path; duplicate abstractions, parallel state, and dual implementations must converge.
- Actively search for existing methods, services, helpers, and behavior paths with the same purpose. If the purpose is the same, reuse the existing capability; do not rebuild a copy under a new name.
- If the old capability serves a narrower scenario but the old and new needs share the same semantics, invariants, and reasons to change, upgrade or generalize the existing single owner
  so the old and new callers use it together; do not keep the old dedicated implementation and add a "new version" alongside it.
- Implementations that are only superficially similar in code but differ in semantics or reasons to change are not forcibly merged; generalization must reduce current duplication, not build a future framework.
- Two methods, condition chains, or transformation pipelines that do the same thing are duplicate paths even if they live in the same file, have different names, or each has only one caller;
  the reviewer should point out the single reusable owner and the direction of convergence.
- Abstractions, branches, fallbacks, retries, optional values, configuration items, and extension points must be backed by current, reachable, verifiable requirements.
- An edge case enters the main implementation only when it belongs to the current contract and occurs naturally; do not add special cases or local optimizations one by one for speculative scenarios.
- Degrading the post-failure state is allowed only when it still has clear value to the user; internal invariant violations and unrecoverable failures should fail fast at the single responsibility boundary.
- Temporary multiple paths must be explicitly named, have an owner and a deletion condition, and be removable as a whole; otherwise they must not coexist.
- This dimension only requires removing or converging complexity; it must not add defensive paths in the name of robustness.

Severity:
- duplicate methods, duplicate paths, or unnecessary complexity that can be locally deleted / generalized / reused → **needs-refinement**
- the same purpose has already formed parallel owners, dual implementations, or branch systems that cannot be locally converged → **abandon**

### 9. Other

This only takes problems that satisfy goal-relevance admission, genuinely affect delivery, but cannot reasonably be placed under dimensions 1–8. When using this dimension, the observation or impact
must explain why none of the existing dimensions applies; if it is merely a naming preference, a style nit, plan completeness, a quality-gate responsibility, or an out-of-goal code observation
outside dimension 6, discard it outright; it cannot be stuffed into "Other".

Severity:
- a substantive problem locally fixable within the current diff → **needs-refinement**
- a fundamental problem with first-party evidence that it cannot be locally salvaged → **abandon**


## Guardrails

The narrative around the diff — commit messages, PR description, code comments, the linked plan — is, for the reviewer, a set of **claims to be verified**, not a source of facts. Adopting the author's explanation of the change and then "checking" it means being anchored to the author's perspective; the value of an independent review comes precisely from not sharing the author's derivation path.

1. **Understand the problem independently first, then compare with the implementation** — at the start of the review, first read the full current state of the changed files and their callers and tests (not just hunks; for cross-module topics, also the applicable architecture sources of truth), independently understand the problem this change solves and its constraints, and if needed derive yourself "roughly what shape a reasonable implementation would take", then compare with the diff: if the implementation differs significantly from the independent derivation → chase what supports the difference (supported by the linked plan / `已归档的决策` → it stands; no support found → finding)
2. **The author's narrative is not evidence** — a commit message saying "fixed X", a comment saying "this is never null here", a PR description saying "behavior unchanged" — always go back to the code and tests to confirm; judgment can only rest on the actual behavior of code and tests. Quantities/limits, ordering and lifecycle guarantees, and universal "only/always/never" assertions (whether in comments, the PR description, or the linked plan) are the riskiest of these claims — verify each one, and for those that cannot be verified, suggest softening the wording rather than assuming they hold
3. **Read the code before concluding** — if the diff says function A calls function B, Read function B to confirm its signature and behavior.
4. **Do not guess** — if unsure, Read the source file.
5. **Write test files only for behavior claims** — correctness (dimension 2) / security (dimension 3) / Bug (dimension 7) suspicions may create new test files or add your own cases to existing test files, and run tests narrowly to prove the hypothesis; the other dimensions are read-only and proven by source citations. Production code, configuration, build scripts, docs, and plan files are never touched, git write operations are all forbidden, and quality gates (lint / format check / coverage / build) are never run (see "Verification boundary").
6. **Resolve conflicts by credibility ordering** — actual behavior of code and tests > the repository's architecture sources of truth / official documentation > plan / PR description / commit messages / code comments. When a lower source conflicts with a higher one, judge by the higher one, and write the conflict itself as a finding.
7. **Distinguish facts from preferences** — architecture violations are facts; naming style is preference. Only facts can yield abandon.
8. **Give concrete suggestions** — cite file paths and line numbers, and explain how it should change.
9. **Reviewers are isolated from each other** — when multiple reviewers run in parallel, each reviewer reads only **its own lane's** round history and must not read other reviewers' review.md or any round artifacts (`temp/review-pr/<branch>/<other lane>/`). The independent perspective is by design: reading others' findings causes mutual anchoring, and N reviewers degrade into one. Lane findings converge only on the code author's side.
10. **Do not suggest writing back to plan documents** — a plan document is a one-off consumable once executed; it is not maintained or written back. Suggestions like "update the plan to match the implementation" or "record the deviation back into the plan" are never allowed; if code violates the locked goal, non-goals, or archived decisions, review only the code and do not request plan changes.
11. **Verification commands themselves must be verified** — for commands the PR description/plan claims were run, confirm they actually exist, run against the correct crate/
target/workspace, and actually select the target tests. Rust crate tests, Python script tests, and skill script tests are different lanes; one lane
exiting 0 does not prove another is covered, and zero tests selected is not valid evidence either.

## Calibrating the adversarial posture (--devils-advocate): skeptical, not contemptuous

Standard review already requires treating the narrative around the diff (commit messages, PR description, comments, linked plan) as claims to be verified (see Guardrails). The adversarial posture strengthens this: disbelieve by default, actively falsify, and specifically seek the failure paths the code avoids. But "stronger skepticism" slides very easily into "contemptuous dismissal", which must be avoided.

**Skepticism = seeking truth more deeply, not denying faster.** It cuts symmetrically both ways: neither credulously accept the claims of code/comments/PR description, nor credulously accept the counter-explanation you use to overturn them. Like the UFO skeptic who blurts out "that's obviously a balloon" — grabbing the most convenient deflating explanation and calling it done — that is not skepticism; it replaces evidence with an equally unverified counter-claim. Credulity and contempt are two faces of the same disease: both substitute a convenient story for facts.

Therefore, in the adversarial posture:

- **Construct the strongest counter-argument for every design choice in the diff first**, but it only stands if it refutes the original implementation on **code you have actually read**; the counter-argument must pass the same verification bar, and if it cannot refute, it cannot refute
- **Treat every claim outside the code as unproven**, and go back to code and tests to confirm (quantities/limits, ordering and lifecycle, universal "only/always/never" assertions are the riskiest; verify each one against the source)
- **Specifically look for boundaries the tests avoid, states the mental walkthrough never passes through, resources acquired but never released, simpler rival implementations it did not consider, hidden coupling**
- Counter-arguments must pass the proof bar too: **if it can be written as a test, write it; only a red run counts as a refutation**; a counter-argument that cannot be verified or does not run red **cannot** become a finding — the original implementation stays, or it is marked "pending evidence", rather than assuming it is broken
- Still bound by Guardrails and "Nitpicking versus real findings": adversarial means harder questioning and deeper verification, not more noise — no nit-spamming, no style preferences, every finding passes your own scrutiny first. A clean verdict must be marked "challenged" + what was tried

## Verification boundary: prove behavior suspicions with tests, do not run quality gates

The review side is not read-only, but write permission has a clear scope. **Test-based proof applies only to claims about runtime behavior — correctness (dimension 2), security (dimension 3), Bug (dimension 7).** The other dimensions (architecture, logging, naming, single purpose, over-engineering) are judged by reading source, with path:line and source observations as evidence; they write no probes, and the `已证明` / `未证明` marks do not apply to them — an architecture finding is not something a unit test can falsify in the first place.

In these three dimensions, **writing a test is the reviewer's default action, not an option**: any thought of "I suspect the behavior here is wrong" has as its first action writing it as a test and running it, and only then deciding whether to write a finding. This is the anti-entropy mechanism of this flow — the gap between "cannot prove" and "proven" is exactly where regressions enter the codebase; a red test simultaneously gives the root cause, the reproduction, and the fix acceptance criterion, while a paper suspicion gives none of the three.

**Review quality depends on goal-related coverage and evidence, not on the number of findings or red lights.** All write and run requirements below first pass "Goal-relevance admission".

### Do

- **Write behavior suspicions as tests**: create a new test file, or **add** cases to an existing test file (the boundary for changing existing cases is below). A behavior suspicion that cannot be written as a test usually has not been thought through yet — think it through first, rather than writing a finding first.
- **Run tests, and always narrowly**: use `just test-one <filter>` (a single cargo nextest filter) for Rust tests, and
  run a single Python test module directly (`python3 <path to test_*.py>`) for script tests. Keep the scope to this
  suspicion and directly related existing tests.
- Read-only git / grep / file-read queries — they remain first-party sources for the review.

**But do not write tests to pad the count.** A probe must have a meaningful failure mode: it should turn red if the implementation is wrong. Asserting the harness's own output, asserting that a mock you just configured was called, or re-asserting the same behavior under a different wrapper does not count as proof.

### Do not

- **Change any non-test file**: production code, configuration, build scripts, docs, and plan files are never touched. Adjusting the implementation to make a test go red is manufacturing your own evidence; the hypothesis does not stand.
- **Git write operations**: `add` / `commit` / `checkout` / `stash` / `rebase` are all forbidden. Probe tests stay in the working tree for the author to handle (see below).
- **Run quality gates**: `just lint`, `cargo fmt --check`, `cargo clippy`, `just build`, and `just ci` / `just check` are all forbidden, **including** filtered forms. These are **the responsibility of CI and `gate-and-fix`**; a reviewer rerunning them does not change the gate verdict.
- **Run unfiltered full test suites**: `just test`, `cargo nextest run` without a filter, and `just maintenance-test`.
  Live end-to-end checks (`just e2e`) do not enter the review side.

### Probe test naming and ownership

- The file name describes the module under test and the specific behavior/scenario, following the directory and suffix conventions of nearby tests; when a suitable file already exists and there is no parallel
  writer, prefer adding cases to it. Case names describe the input condition and expected behavior, so a maintainer who doesn't know the reviewer can understand them.
- Never put model names, reviewer/lane, dimension group, round, date, or temporary numbers into test file names, module names, or case names.
  For example, `persistence_claude_1_round_02.rs` is not acceptable; to verify recovery after a save failure, name it
  `persistence_save_recovery.rs`, with a case named `preserves_last_saved_session_when_save_fails`.
  Names must match the actual assertions; do not invent behavior to fit the example.
- Parallel isolation relies on explicit file write ownership: the same main agent assigns non-overlapping test files before dispatch; an independent lane first checks
  existing probes in the working tree and does not contend when there is already a writer. When an independent file is needed, refine the name by a different real scenario, without a lane
  suffix or random number. When a test for the same behavior already exists, reuse and verify it; do not create a synonymous duplicate case.
- The mapping between lane, round, source, and test path is written only in this lane's review artifact; it does not pollute long-term test naming,
  and no new persistent ownership file is created.

### Probe test lifecycle

Probe tests **stay in the working tree, uncommitted**, and the author decides whether to adopt, rewrite, or delete them. Accordingly:

- **Do not overwrite another lane's probe.** Follow the explicit file write ownership from the previous section; when changing an existing test file, only add or remove your own cases, never others'.
- **Changing existing test files has a cost**: existing tests are part of the author's verified baseline. Deleting or weakening an existing assertion is not a review action; it is changing the baseline — only adding cases is allowed, plus correcting an assertion that is genuinely wrong (in which case the correction itself must become a finding explaining what was wrong with the original assertion).
- **A green probe is not a finding.** If it runs green after writing, the current behavior is correct for that input: withdraw that suspicion and open no finding in any dimension for it. The probe stays in the working tree, and one line is recorded in review.md "本轮探索区域 / 运行的测试" (this round's exploration areas / tests run), for the author to decide whether to adopt or delete it.
- **Never leave a red test without a finding.** A red run must become a finding in the corresponding dimension, explaining which line holds the root cause.

### Suspicions that cannot be proven may still be reported, but must be marked "unproven"

Raising a suspicion is not wrong in itself. Rules:

- What can be proven with a Unit / Integration test **must be proven**, and must not be reported in suspicion form.
- **Behavior that can only be confirmed by manual verification in a real running app, or by an agent-driven app session, may remain a suspicion.** The review side does not run E2E, and parallel lanes cannot each start their own app session.
- The evidence field states the proof status: proven ones write `已证明：<test name @ test file relative path> — <failing assertion>`; unprovable ones write `未证明`. **There is no need to explain why it cannot be proven, nor to list what was tried.**
- Unproven wording is "cannot rule out X" rather than "this is wrong", and the impact is written conditionally.
- **An unproven suspicion does not block Ready, does not trigger a new round**, and cannot by itself be the basis for changing verified existing behavior.
- Unproven is not an escape hatch: what can be written as a test must be written as a test.

### Proof and recording

Findings still stand by **reading source** — all-green gates could never prove the absence of problems in this guide's 9 dimensions (most of them are not expressed by any gate at all). Tests are the means of upgrading a "suspicion read from the code" into a "proven fact", not a substitute for reading.

When a red run occurs, the finding's **evidence** field records: **test name + test file relative path + the observed failure** (assertion, expected value, and actual value). Do not paste large chunks of runner output, and do not record runner timing or environment noise.

A suspicion that did not run red is **not** a behavior-changing finding: withdraw it if green; if it can only be verified manually or by an agent driving a real running app, report it marked "unproven" per the previous section.

This section constrains the **review side**. The author side (`/address-review-comments` APPLY) must run verification for the touched modules after fixing, and full gates belong to CI and `gate-and-fix`; that is a separate matter.

## Nitpicking versus real findings

The review agent must distinguish nitpicking from real findings. The severity mapping is defined by this guide; the review agent must not escalate or downgrade it on its own.

**The following is nitpicking (ignore or omit):**
- suggesting a replacement name when the current name is already descriptive enough
- suggesting optional logs, comments, docstrings
- proposing a tiny refactor that affects correctness neither when done nor when skipped
- suggesting error handling / empty-value fallback / fallback for a scenario that cannot happen, or whose failure is app-fatal (no meaningful degradation path) — the former should fail fast in place (`panic!` / `unreachable!` / an error returned at the owner), the latter should be allowed to crash at a reasonable boundary; demanding local fallback is reverse over-defense (note: when you find **existing** fallback of this kind, the direction is the opposite — make it a dimension 8 finding requiring removal)
- commenting on code the diff does not touch when no architecture issue is involved

**The following is not nitpicking (must be acted on):**
- any item in this guide's 9 review dimensions whose severity maps to abandon
- broken tests or API mismatches
- correctness bugs
- security vulnerabilities
- severity mappings explicitly defined in this guide's 9 review dimensions

**Key principles:**
- This guide's severity mapping is final — do not escalate style preferences to abandon, and do not downgrade abandon criteria to needs-refinement
- If a finding cannot map to dimensions 1–8, first verify it against dimension 9's strict admission; if it still does not qualify, it is nitpicking
- early return is always better than nested conditions — this is not a style preference

## Output format

**The review output is a forensic evidence report (CSI report), not a verdict document, and not a persuasive essay.** Each finding states facts and lets the author make their own disposition judgment from the evidence — do not conclude for them, and do not push them toward a particular disposition.

### Detailed analysis

Organized by dimension, each finding is an unlabeled statement of fact:
- **位置** (location): file path and line number
- **维度** (dimension): one of this guide's 9 review dimensions (when using "Other", explain why none of the existing dimensions applies)
- **观察** (observation): what it is — state facts, without slipping in disposition suggestions
- **证据** (evidence): the code / mechanism / cross-references proving the observation above, listing path:line one by one
- **影响** (impact): what happens if it is not addressed
- **可能的修复** (possible fix) (optional, provided as information, without pressing for adoption; Bug findings must include reproduction test / fix suggestion / regression test)

**Never put a disposition or severity label on an individual finding** — `阻断` (blocking) / `建议采纳` (recommend adopting) / `abandon` / `needs-refinement` / `critical-high-medium-low` are never written. "Recommend adopting" literally means "I recommend you adopt this", pushing the author toward acceptance before their own judgment; "blocking" is a verdict passed on their behalf. This guide's per-dimension "severity" mapping is only the basis for the reviewer's **internal** judgment, used only to aggregate the **single** final verdict at the end, and is never exposed on each finding.

### Final verdict

The last line must be one of:

- `Ready` — can be merged, and meets the Ready hard threshold in "Three verdicts".
- `Needs Refinement` — can be merged, but this round listed SUBSTANTIVE findings (all listed before the last line)
- `Abandon` — should not be merged (all blocking reasons listed before the last line)

The output ends with the verdict line; nothing is appended after it.

## Additional resources

Internal project references:

- **Module architecture**: code, imports, call sites and tests — module responsibility, dependency direction, public API, test scope
- **Cross-module SOT**: the repository's architecture sources of truth under `docs/` and `skills/skill-architecture.md` — test layering, quality gates, runtime topology, etc.

The review agent may use WebSearch/WebFetch or context7 to consult official documentation as needed:

- **Rust**: https://doc.rust-lang.org/book/
- **Rust standard library**: https://doc.rust-lang.org/std/
- **Cargo**: https://doc.rust-lang.org/cargo/
- **Python**: https://docs.python.org/3/
