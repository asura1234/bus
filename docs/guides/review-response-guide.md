# Review Response Guide (Author Side)

`review-plan`, `review-pr`, and execute-plan task review produce evidence-backed findings. PR reviewers may also leave uncommitted red probe tests. This guide governs the other side of that process: the author verifies, adjudicates, groups, and remediates one or more compatible review artifacts.

Reviewer and author are peer decision-makers with different responsibilities. A reviewer identifies a problem and may suggest a repair. The author independently decides whether the claim is true, whether it belongs in scope, and what repair is correct, then owns the actual patch. A later review validates the result. Review is input, not command; convergence is measured by evidence and final behavior, not by literal compliance with a proposed fix.

The author's value is independent judgment. Automatically land only verified, admitted issues. Record misreads, scope expansion, over-engineering, conflicts with locked choices, and true open decisions as the fixed REJECT or FLAG disposition rather than silently accepting or dropping them. Deduplicate repeated root causes.

This guide is language-independent. It defines judgment; execution order belongs to `address-review-comments`.

## Guardrail: verify before adjudicating

Every finding is checked against first-party reality before disposition:

1. Read current source, plan text, tests, and archived decisions. A confident review sentence is not proof.
2. Multiple lanes reporting the same claim increase investigation priority but never replace verification; they can share the same misread.
3. Separate the problem claim from the proposed remediation. Independently establish root cause, postcondition, behavior owner, upper-level invariants, and regression evidence. Then evaluate the reviewer's suggestion as one candidate that may be accepted, narrowed, broadened, or replaced. A patch that removes a local symptom while breaking lifecycle, visibility, ownership, or adjacent transitions is wrong even when the finding is true.
4. Resolve conflicts by authority: current source and tests; repository/module SOT and authoritative documentation; plan/PR/commit prose; review wording.
5. Repository writes are allowed only for APPLY. REJECT, FLAG, and HOUSEKEEPING never modify repository content. Plan and PR modes land by default; task mode always uses `--no-commit-and-push` and leaves owner-local changes for execute-plan's final landing.
6. A true claim is not necessarily a safe behavior change. PR mode usually follows manual or automated validation, so existing behavior is a protected baseline. Changing it requires positive evidence: a reachable input and wrong output that prior validation missed. Without that evidence, strengthen the current contract with a test or FLAG the behavior choice. Task mode occurs before final validation and does not inherit this protected-baseline assumption.
7. Every deduplicated finding and consistency item passes the goal/scope gate before repair design:
   - work outside the locked goal -> `scope-change`;
   - work toward an explicit non-goal -> `scope-change`;
   - expanding, shrinking, overturning, or redefining the goal -> `violates-stated-goal`;
   - speculative robustness outside the goal contract -> `over-engineering` or `robustness-not-in-goal`;
   - an incremental-round finding first raised against unchanged material with no delta relationship -> `review-scope-violation`.
   These are REJECT, never APPLY. A real defect may still be outside this task's authority. A red test, multiple agreeing lanes, earlier APPLY, or permission to include incidental work does not grant scope.
8. Verify current HEAD. Historical review may describe an older snapshot. A claim repaired before adjudication is `already-addressed`, with its current location. Locate by symbol and context rather than stale line number.

## Verification and adjudication authority

The main agent chooses the orchestration. It may verify claims serially, delegate a bounded set of read-only fact questions, or combine both, based on claim count, shared files, and context cost. No specific fan-out shape is mandatory.

These invariants always hold:

- Every canonical claim receives exactly one first-party truth assessment before adjudication: `SUPPORTED`, `CONTRADICTED`, or `INCONCLUSIVE`, with evidence location and uncertainty.
- Verification and adjudication are different authorities. A verifier answers only the atomic fact question. Goal/scope admission, disposition, conflict resolution, and repair direction belong only to the main agent. `SUPPORTED` is not automatically APPLY and `CONTRADICTED` is not automatically REJECT.
- Truth assessment applies to the problem claim, not the suggested repair. APPLY means the problem should be fixed, not that the reviewer's patch should be used.
- Every disposition in the triage ledger carries a first-party citation: path:line, exact document section, or command and output. No citation means the claim was not verified.
- Delegated verifiers are repository- and Git-read-only. They do not write, stage, commit, push, decide disposition, propose a patch, or expand scope.
- No repository write occurs until every claim has been adjudicated.

### Bounded verification

Read enough source, callers, and contracts to establish the claimed behavior, its owner, goal relationship, and concrete consequence. REJECT is a disposition, not permission to skip verification based on a title or path. The same file may contain goal-related and unrelated behavior.

Do not expand verification into exhaustive input enumeration, unrelated bug discovery, or repair design. When evidence is insufficient, record `INCONCLUSIVE` and the missing evidence. Preserve truth and scope separately: `SUPPORTED + out-of-goal repair = REJECT(scope-change)`. A genuine unresolved product or architecture choice is `requires-developer-decision`; known out-of-goal work is not promoted to FLAG merely because it would be useful.

## Input sanitation, consistency drift, and task isolation

- Run `prepare_review_input.py` before reading review content. It validates mode and target identity and emits only the two actionable sections. Structured review may come from these skills; free-form review is also valid with `--free-form-file` and explicit mode. Free-form input lacks round provenance and SCOPE_HASH, so it cannot be classified as `review-scope-violation`; its locked goal must be supplied explicitly.
- The non-blocking consistency list is still actionable input. A verified in-scope wording, naming, comment, or documentation item is APPLY-SAFE. False or out-of-scope drift receives the same fixed disposition discipline. It does not change the prior review verdict or trigger another round by itself.
- Triage metadata begins with `**模式**：plan|pr|task`. Task triage also records exact plan and task identity and lives under the task namespace. Task APPLY stays within the declared owner; repairs that require another owner return to execute-plan for task-graph repair.

## Dispositions: each claim receives exactly one

There are four actions: automatic repair (APPLY), direct rejection with no developer action (REJECT), escalation for developer decision (FLAG), and internal consolidation (HOUSEKEEPING). A disposition has a fixed action. Judgment decides which disposition applies; routing after that is table lookup.

Read only the named judgment sections from reviewer guides. Do not import their review-production steps or issue a reviewer verdict from the author side.

| Disposition family | Judgment source of truth |
| --- | --- |
| APPLY-SAFE / APPLY-BEHAVIOR | code-review guide, “Review context: existing behavior is a verified baseline” |
| `over-engineering` | code-review KISS/YAGNI/dimension 8/nit boundary; plan-review over-spec boundary |
| `scope-change` | code-review goal-relevance admission and dimension 6; plan-review multi-purpose rule |
| `already-addressed` | code-review scope; in plan mode, current plan and archived decisions |
| `review-scope-violation` | closed-world convergence rules in plan/code/task guides |
| `violates-stated-goal` | plan-review locked workflow and goal rules |
| `implementation-detail` | plan-review admissibility stop rule; code-review nit boundary |
| task evidence and owner rules | task-review evidence and file-isolation sections |

### APPLY: automatic remediation

The author independently derives every repair from first-party context. For implementation or behavior, identify root cause, required postcondition, behavior owner, preserved upper-level invariants, and regression evidence that proves both the defect is gone and existing correct behavior survives.

#### APPLY-SAFE

- `valid-issue` without behavior change: dead-code removal, comments/naming/spelling, tighter visibility, logging, unit tests, documentation, or replacing an existing impossible-state fallback with fail-fast.
- `strengthen-with-test`: the review raises a behavior concern, but current behavior is verified correct and simply lacks protection. Add a regression test for the current contract rather than changing behavior.

#### APPLY-BEHAVIOR

- `valid-issue`: the claim is verified, supplies positive evidence of a reachable wrong result, is in scope, and can be repaired without over-engineering.
- `partial`: the claim is true but the suggested patch is too broad, too narrow, only treats a symptom, or violates existing patterns. Implement the author's smaller root-cause repair while preserving upper-level invariants.

Without positive evidence that protected PR behavior is wrong, do not use APPLY-BEHAVIOR. Use `strengthen-with-test` or FLAG `behavior-change-unverified`.

### REJECT and FLAG: no automatic repository changes

Every non-APPLY claim remains visible. The mapping is fixed.

REJECT requires no developer action and is recorded compactly:

- `over-engineering` / `robustness-not-in-goal`: adds an abstraction, state, branch, or failure path unsupported by current goal and reachable requirements.
- `claim-not-true`: first-party source contradicts the claim.
- `already-addressed`: the issue is real historically but absent at current HEAD.
- `scope-change`: the requested repair does not serve the locked goal or advances a non-goal.
- `violates-stated-goal`: the finding changes the developer's locked assignment.
- `review-scope-violation`: a closed-world follow-up introduces a finding against unchanged, unrelated material with clear provenance.
- workflow-structure rewrite: changes the fixed one-shot plan/verification/PR cadence.
- pure `contradicts-archived-decision`: conflicts with a locked decision without disproving its factual premise.

FLAG requires developer decision and includes complete evidence:

- `behavior-change-unverified`: asks to change protected PR behavior without positive evidence that current behavior is wrong and cannot be resolved by strengthening tests.
- `conflicting`: two verified lanes require mutually exclusive outcomes.
- `requires-developer-decision`: a genuine product or architecture choice cannot be resolved from source and locked decisions.
- archived-decision premise challenge: new evidence undermines the factual premise of the decision rather than merely disagreeing with it.

Specific rules:

- `scope-change` remains REJECT even when the defect is real, the test is red, or the work seems useful. The developer may authorize it separately later.
- `review-scope-violation` requires clear provenance. When provenance is ambiguous, adjudicate normally rather than using uncertainty to discard a legitimate issue.
- `contradicts-archived-decision` becomes FLAG only when evidence challenges the premise. The author neither reopens nor defends the decision automatically.
- `conflicting` presents both verified sides and the tradeoff; the author does not choose silently.

### HOUSEKEEPING

- `duplicate-root-cause`: merge into one canonical claim and record corroborating lanes. Repair or escalate only once.
- `implementation-detail`: plan-review detail below macro level or code-review style outside every review dimension. Record internally; it is not an actionable finding.

## Cross-lane adjudication

- Deduplicate by actual root cause, not wording.
- Detect mutually exclusive instructions and FLAG them.
- Reject scope expansion and unsupported robustness even when they appear comprehensive.
- Treat corroboration as investigation priority, never as truth.

## Taking ownership of PR reviewer tests

Reviewer tests are pending regression evidence, including red untracked files and new cases in existing test files. Match each test to its claim, actual assertion, and worktree hunk before modification. Red does not automatically prove the reviewer's expected behavior.

For APPLY, include the corresponding test in the repair allowlist and narrowly prove red-to-green after fixing the root cause. Keep accepted green coverage tests too. Rename or merge them only to match actual semantic behavior. Never manufacture green by deleting, skipping, weakening, or mocking away the assertion. If the original assertion is wrong, rewriting it requires independent first-party evidence and a triage note.

PR mode lands accepted tests with the repair through commit-and-push, including previously dirty or untracked reviewer files. Verify every adopted hunk entered the commit and remote branch. Do not commit rejected, disputed, or unrelated probes automatically. `--no-commit-and-push` still forbids landing.

## Landing and boundaries

- Land only verified APPLY claims and fix root causes rather than copying proposed patches.
- Never land FLAG automatically.
- A repair must not reactivate flagged/merged claims, introduce conflict, or expand change surface unnecessarily.
- Task-mode “landing” means owner-local worktree changes and refreshed gate evidence only; Git landing remains forbidden.
- Plan and PR modes use the repository's commit-and-push workflow when landing is enabled. Respect the current Bus branch policy and use an explicit refspec.
