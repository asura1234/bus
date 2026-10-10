---
name: address-review-comments
description: Author-side adjudication and handling of plan/PR review comments, including fixed whole-file chunks with the same --scope as review-pr. Deterministically sanitize inputs and deduplicate by root cause, then obtain first-party verification evidence for each claim (the main agent decides whether to self-verify or delegate to read-only subagents), decide APPLY / REJECT / FLAG / HOUSEKEEPING centrally, and finally remediate grouped by related root cause. Plan and PR modes land through commit-and-push by default. Use when asked to address or respond to review comments.
---

Respond to `/review-plan`, `/review-pr`, or any free-form review. Reviewer and author are peer decision-makers in a convergence process: the reviewer raises a problem and may suggest a repair, the author independently judges the problem and the correct repair, and a later reviewer verifies the result; this skill is author-side adjudication + remediation and does not treat review as a command.

Before execution, read completely:

- [Review Response Guide](../../docs/guides/review-response-guide.md): the SOT for verification and disposition judgment;
- [guide.md](guide.md): judgment **specific to the GitHub lane** (round identity, location basis, clean re-review, completion marker,
  ledger leak scan). Needed only when taking PASS 0a; other modes need not read it;
- the **specific sections** that guide's disposition classification table names for this run's mode: read only those sections, not the whole review guide.
  `plan-review-guide` / `code-review-guide` are **reviewer-side** documents; their
  review scope, three verdicts, adversarial stance, output format, and similar sections describe how findings are produced, which the author neither performs
  nor outputs; reading the whole document only tempts the author to apply reviewer actions (for example issuing a verdict on the reviewer's behalf);

```text
INPUT = [plan | pr] [--round latest|N] [--github] [--pr <n>] [--review-file <path>]...
        [--free-form-file <path>]... [--label <name>]... [--no-commit-and-push]
        [--scope <FILE_LIST|chunk.json>]

HARD RULES
- Automatically modify only claims finally adjudicated APPLY; REJECT / FLAG / HOUSEKEEPING do not change the repository,
  except that PR mode lands or removes every reviewer-added test (PASS 4); that is landing, not a repair.
- The main agent exclusively owns deduplication, cross-claim comparison, disposition, triage, remediation grouping, and landing.
- Evidence gathering answers only local facts: no adjudication, no repair suggestions, no patches, no repository changes; disposition, conflict resolution, and repair direction belong to main alone.
- Claim truth, goal scope, and repair design are judged separately: `SUPPORTED` does not approve the scope or the repair; main first rules on scope, then independently derives the repair for admitted claims.
- **Orchestration is main's own decision**: verify each claim itself, hand some to read-only subagents, or mix, depending on claim count, whether claims share files, and remaining context. This skill prescribes no shape and does not assume parallel is faster (reasoning in the guide's "Verification and adjudication authority").
- Unified adjudication barrier: no write operation of any kind until every claim has a first-party truth assessment and main has completed unified adjudication.
- **Never run repository-wide gates; run only validation within the diff.** Forbidden: `just ci`, `just check`, `just test`,
  `just lint`, an unfiltered `cargo test` / `cargo nextest run`, and any other full-tier run;
  only diff-only / scoped forms are allowed:
  `just test-one <filter>`, `python3 -m pytest <exact test file>` / `python3 -m unittest <exact module>`,
  `rustfmt --check --edition 2021 <changed .rs paths>`.
  Reasoning and execution details are in PASS 4 "Validation covers only the diff".
- plan/pr invoke commit-and-push by default.

========== PASS 0: RESOLVE & SANITIZE ==========

IF --review-file / --free-form-file is given explicitly: canonicalize in argument order and deduplicate.
ELSE: only legacy plan/pr lane discovery is allowed.

IF --scope: mode is pr; use exactly the same file list / manifest as review-pr (format and permissions
in that skill). Canonical files + test_files must match the review header's SCOPE_HASH. Findings may
reference unchanged chunk files; absence from <base>...HEAD is not a scope violation.
Read the declared whole-file target and production-review Goal, not a feature-diff-only assignment.
Do not mix branch artifacts, other chunks, or plan reviews. No GitHub diff-lane fetch occurs in scope
mode; --github / --pr are not chunk inputs. Use explicit --review-file, or discover only
temp/review-pr/<slug>/scopes/<SCOPE_HASH>/*/round-*/review.md with the same latest / round rules.
No work outside that chunk is admitted merely because it is a direct dependency.

========== PASS 0a: GITHUB LANE (mode == pr, no --scope, and no explicit --review-file / --free-form-file) ==========

IF mode == pr AND no --scope AND no explicit --review-file / --free-form-file:
  Read(skills/address-review-comments/scripts/github_lane.md) completely and execute its procedure.
  Continue below for sanitization unless the procedure returns normally or STOPs.

RUN `python3 skills/address-review-comments/scripts/prepare_review_input.py \
  [--review-file <path>...] [--free-form-file <path>...] [--mode plan|pr] \
  [--label <name>...] [--scope <FILE_LIST|chunk.json>] --output <run-root>/sanitized.md`
-- With `--github` this is verbatim `--review-file <roundDir>/review.md --mode pr`.
IF exit != 0: STOP; never bypass the script to read the original review.

Pre-migration review files are immutable historical evidence: pass them as `--free-form-file <path> --mode plan|pr`; never rewrite one to satisfy the parser.

Legacy discovery rules:
- The branch slug is the same as the review skills': detached uses `_detached_`; otherwise characters outside `[A-Za-z0-9_-]` are replaced with `-`;
- plan lanes come from `temp/review-plan/<slug>/*/*/round-*/review.md`, PR lanes from `temp/review-pr/<slug>/*/round-*/review.md`;
- An explicit mode uses only the matching lanes; when only one kind exists it is selected automatically; when both or neither exist, STOP; each lane takes the requested round or its latest valid round.
- **A lane whose verdict is Ready and whose sync list is empty does not enter this run**: that lane has signed off and left nothing to address.
  Lanes whose rounds stopped at different HEADs are not a problem: PASS 2 requires first-party evidence for every claim, so
  claims already fixed get `CONTRADICTED` there, which is exactly why the evidence step exists.

Read only sanitized's "新问题与建议" and the complete "同步清单（CONSISTENCY drift，非阻塞）":
- structured lanes must agree on mode, target, base, locked goal, and plan identity; any mismatch STOPs;
- structured chunk inputs require the matching --scope; sanitized provenance carries the canonical
  SCOPE_HASH, whole file list, and exact test allowlist. Cross-chunk / branch mixing fails closed;
- free-form requires an explicit mode; its round is `n/a`; without --scope its SCOPE_HASH is `n/a`.
  An explicit --scope supplies file membership, not prior-round provenance; the closed incremental
  gate remains undecidable. The locked goal must be given explicitly by the caller/developer, STOP
  if it cannot be obtained, and never guess it from the diff, commits, or PR description;
- free-form lacks round provenance and SCOPE_HASH, so the closed incremental gate (d) **has no mechanical criterion available**. This is not "this round happened not to hit it"; the gate is undecidable at this entry point and must be written explicitly in triage as one line `**闸 (d)**：free-form 输入不可判`, so the gap leaves a trace instead of silently taking effect. The guardrail's gate (a′) already has a similar skip clause, and gate (d) is recorded following the same precedent;
- plan reads the full plan text and archived decisions; PR reads `<base>...HEAD`, the locked goal, the optional associated plan, and round provenance, plus the complete files and direct dependency context each claim needs;
- scoped PR reads the committed whole chunk files and its previous-round snapshot / delta instead of
  using the branch diff as finding admission; all evidence still targets current HEAD. Closed-round
  gate (d), truth assessment, disposition, and author-selected repair rules remain unchanged;
- when plan/pr landing is needed while on master, create a feature branch first.

run root: plan/pr is `temp/address-review-comments/<slug>/<YYYYMMDD-HHmmss>/`.
With --scope it is `temp/address-review-comments/<slug>/scopes/<SCOPE_HASH>/<YYYYMMDD-HHmmss>/`;
put **范围哈希**：<SCOPE_HASH> in triage.md so author decisions stay tied to the chunk.

IF mode == pr: combine the test paths in sanitized findings with the current Git working tree to identify new test files the reviewer left and
new cases in existing test files; record the source claim and original red/green state; stay read-only at this point and do not treat them as unrelated dirty files
merely because they are uncommitted or currently red. Adoption, validation, and landing follow review-response-guide "Taking ownership of PR reviewer tests".

========== PASS 1: ATOMIZE & DEDUPE (main only) ==========

1. Extract substantive findings and sync-only assertions; record source lane/round/file/id, location, dimension, observation, evidence, impact. The reviewer's possible repair is recorded separately as a non-binding candidate and must not be mixed into the problem claim or used as evidence-gathering input.
2. When one finding contains several independently true facts, atomize it first; every source assertion must belong to exactly one canonical claim.
3. Deduplicate across lanes by actual root cause and keep corroborating lanes; mutually exclusive guidance is only marked as a candidate conflict and must not be adjudicated early.
4. After deduplication, record for each canonical claim: claim id, source assertions, a precise context pointer (if you can give `path:start-end`, do not give only a file name: evidence cost is mostly spent reading whole files), and the **single** atomic fact to verify for that claim.
5. Atomization follows "can it stand independently", not "finer is better": assertions with the same root cause, same file, and same repair site merge into one claim. Splitting too finely makes the same context get rebuilt repeatedly without improving accuracy.
6. An empty claim set is valid; mechanical output continues with `0/0`.

========== PASS 2: VERIFY EVERY CLAIM ==========

Every canonical claim must receive one first-party truth assessment (`SUPPORTED | CONTRADICTED |
INCONCLUSIVE`) with the evidence location and uncertainty; if evidence is unavailable, say so; do not guess.
The assessment answers only whether the problem claim holds; it does not evaluate or endorse the reviewer's candidate repair.
PR evidence also establishes the behavior owner, goal relationship, and concrete consequence so main can independently rule on scope; follow the guide's "Bounded verification":
never skip verification based only on a title or path, and do not expand into exhaustive input enumeration or repair design for unrelated subsystems.

**How evidence is gathered is orchestrated by main**: read source/plan/archived decisions and run commands for each claim itself, hand some to read-only
subagents, or mix. The choice depends on claim count, whether context pointers overlap, and main's remaining context:
when claims cluster in the same few files, doing them back to back yourself is usually faster because the context is built only once.

Hard constraints when delegating (however many groups and however split):
- subagents are read-only for the repository and Git: no business-file writes, no add/commit/push/stash/checkout;
- report only first-party facts and the closed-set conclusion above: no disposition, no repair direction, no new issues discovered in passing;
- main passes the locked goal, non-goals, and the bounded fact question; the subagent reports the dependencies and consequences it saw without expanding into a module audit;
- they must not cite each other's findings: using A's evidence to shore up B is the same contamination as reading someone else's report;
- when context is insufficient, evidence is unavailable, or the read-only boundary blocks them, report STOP truthfully for main to handle.

Chat summaries are not evidence: every conclusion must land on a `path:line`, a section name, or a command and its output for the ledger to cite.
Any unresolved STOP blocks adjudication and remediation.

========== PASS 3: ADJUDICATE (main only) ==========

For each claim, main:
1. rechecks whether the evidence is first-party, relevant, and sufficient, and whether it truly supports the truth assessment;
2. first independently applies the guide's GOAL & SCOPE GATE; PR rules on scope per Goal-relevance admission, then considers archived decisions, mode, and the behavior-evidence bar;
3. compares all candidate conflicts; only main may judge `conflicting` or resolve a conflict with first-party facts;
4. chooses exactly one guide-defined disposition and enters APPLY / REJECT / FLAG / HOUSEKEEPING per the fixed mapping.
5. for each APPLY, independently determines the repair direction from first-party context; when implementation or behavior is involved, first make explicit the actual root cause, the post-repair condition,
   the behavior owner, the upper-level invariants/adjacent state transitions that must be preserved, and the regression evidence, then test the reviewer's suggestion as a candidate, which may be adopted,
   adjusted, or replaced. When the candidate only fixes a local symptom or breaks an upper-level invariant, choose another approach; when first-party context still cannot converge on a safe direction and it is genuinely
   an open product/architecture choice, change the ruling to `requires-developer-decision` instead of forcing the reviewer's approach.

A PR's `SUPPORTED + out-of-goal repair request` is fixed as REJECT(scope-change) and must not move to APPLY-SAFE, strengthen-with-test, or FLAG because of a red test, multiple lanes, a prior APPLY,
or Good Samaritan permission; the truth conclusion is kept as is.

Write `<run-root>/triage.md`. The first field is fixed as `**模式**：plan|pr`. Free-form input adds one line `**闸 (d)**：free-form 输入不可判`.
Four categories in sections; each item contains source, location, a one-sentence root cause, a **first-party citation**, and the action; one root cause is adjudicated only once. Every disposition in the ledger
must be able to produce a citation: if you cannot write one, it was not verified. An APPLY's action records the repair direction the author independently selected, not the reviewer's
suggestion copied as an instruction.
For PR, each ruling briefly notes in its first-party citation and action the goal relationship and consequence, or the basis for lacking a relationship; sync-only items also get a scope ruling first.

========== PASS 4: GROUP & REMEDIATE ==========

Plan mode fixed state transition: if the status is `create-plan-complete`, change it to `review-plan-in-progress`; if it is already `review-plan-in-progress` or `review-plan-complete`, leave it, and never move back to `create-plan-in-progress`. With no APPLY, the status change still follows the landing rules.

Take only final APPLY claims and form remediation groups by **related root cause + touched files + dependency order**: claims sharing a root cause, sharing files, or depending on each other must be in the same group. Compute an exact file allowlist per group. Implementation follows the repair direction main independently determined; never use the reviewer's original suggestion directly as a task specification.
With --scope each remediation allowlist stays within the chunk's files / test_files. If a verified
repair needs another production or test file, STOP that repair and obtain an explicit new assignment;
do not expand the manifest or the locked Goal yourself. Adopt only probes from the chunk's test allowlist.

IF mode == pr: include the reviewer test files/case hunks for APPLY claims in the allowlist; after the repair, narrowly run those tests and
directly related regressions, confirming red turns green without weakening effective assertions. When landing, commit and push the repair together with the
adopted new tests through commit-and-push, explicitly checking that new untracked files are also in the commit; never commit only the author's own production code.
IF mode == pr: no reviewer-added test stays uncommitted. Each test for a non-APPLY claim is committed green (a red REJECT probe rewritten
to assert the correct behavior when it still adds coverage), folded into an existing test, or removed with the reason in triage.md;
a red FLAG probe is removed with its path, assertion, and red output in the FLAG evidence. Rules: review-response-guide
"Taking ownership of PR reviewer tests". commit-and-push lands them even with no APPLY; after the push,
`git status --porcelain --untracked-files=all -- <reviewer test paths>` must print nothing, else STOP.

main decides itself whether to work serially or dispatch writers: with few groups, overlapping files, or trivial changes, doing it yourself is usually faster; dispatching pays off only when the
allowlists of multiple groups are pairwise disjoint, have no dependencies, and each group's workload is genuinely substantial. In plan mode the same plan file always has a single writer.

A writer brief gives only the root cause main adjudicated, the independently determined minimal repair direction, invariants to preserve, the exact allowlist, prohibitions on Git/out-of-bounds edits/reopening non-APPLY claims/adding speculative fallbacks, and focused tests and scoped lint; it does not give the reviewer's original suggestion as an instruction. Validation must cover both the reported problem and the upper-level invariants to preserve, not merely prove the local symptom disappeared. Shared-output build/coverage/e2e/deps gates are **not run in this skill**: they belong to gate-and-fix before merge. If a writer can only repair by going out of bounds, it STOPs and must not expand scope or change dispositions on its own.

If writers were dispatched, main waits for all of them to return and proves with actual `git status --short` that: the total change is within the union of allowlists, every file has a unique owner, every group's evidence is genuinely green, and the combined tree reopens no FLAG and creates no conflict. STOP/out-of-bounds/red is taken over by main; if it still cannot converge, convert to FLAG and truthfully report the residual delta; destructive rollback is forbidden.

The serial path validates group by group and invokes commit-and-push; the parallel path invokes commit-and-push once after all writers are reconciled. `--no-commit-and-push` forbids add/commit/push and only records the delta and evidence. Plan edits must not write the review process.

**Validation covers only the diff.** This skill's validation obligation ends at "the packages/files that were changed are indeed green", not "the whole tree is
green". The latter belongs to `gate-and-fix` before merge: it runs the full gate set on a clean committed tree and produces a verifiable artifact,
and it is the only thing entitled to claim the PR is ready. So manually running a repository-wide gate here produces information the scoped runs would also give,
costs several extra minutes, and does not bring the branch closer to mergeable: it only does early what `gate-and-fix` will do anyway.
`just test` / `just lint` / `cargo clippy --all-targets` cover the whole repository; those full gates are still executed uniformly by gate-and-fix before merge.

- After changes, run only the touched surface: Rust uses `just test-one <filter>`; Python uses
  `python3 -m pytest <exact test file>` or `python3 -m unittest <exact module>`.
- Feed formatting checks only the changed paths: `rustfmt --check --edition 2021 <changed .rs paths>`.
- When a repair adds a production branch, **reason about which test case reaches it and add that case**; do not rely on running
  the full suite to discover it; after adding it, still run only the scoped tests.
- Any gate not run as a result is named truthfully under "validation" in the final output and must not be blurred into "validated".

========== OUTPUT ==========

IF mode == pr AND the github lane was used in this run:
  **First reply on each finding's own thread**, then post the ledger. For every finding in this round that has an inline thread:
    python3 skills/address-review-comments/scripts/github_review_lane.py reply \
      --pr <pr> --comment-id <that finding's comment id> --body-file <that item's ruling body> \
      --round-dir <roundDir>
  The body states that item's disposition and first-party basis: APPLY states what was fixed and in which commit; REJECT states the first-party contradiction;
  FLAG states the developer's decision. `--round-dir` cannot be omitted: it is the only basis for not resending on a rerun.
  -- This is the channel that lets the reviewer receive the decision (it does not read the ledger itself); reasoning and observations in [guide.md](guide.md)
  --   "Replying to its thread lets it receive the decision".

  Then post the adjudication ledger verbatim back to the PR (for humans, leaving an audit trail):
    python3 skills/address-review-comments/scripts/github_review_lane.py post-triage \
      --pr <pr> --triage <run-root>/triage.md --round-dir <roundDir>
  -- The machine reviewer can re-raise a rejected comment at any time: it has no memory and only sees that the code did not change. With the ledger
  --   on the PR, "why this is not changed" lives in the same place as the code, so the next round (human or machine) can see that reason.
  -- The script first scans for credentials and local absolute paths and blocks oversized ledgers; on a hit it STOPs and a human decides how to redact;
  --   content is not rewritten automatically here, because rewriting would make what is posted differ from the ledger.
  -- `--round-dir` makes the script write `triaged.json` **after a successful post**: the only evidence a round truly finished.
  --   Why `reviewExists` is not enough: [guide.md](guide.md) "The completion marker is written in the last step".

Output mode, target, lanes/rounds, this run's orchestration (self-verified / which were delegated), per-disposition counts, cross-lane merge/conflict counts, changed files, validation (list every scoped command actually run, and explicitly list the repository-wide gates **not run**), landing/no-landing, and the triage path; parallel remediation also reports groups and allowlists.

**Per-lane unique claim count** takes its own line, in the form
`lane 独有：default: 1  github: 2  总计: 10`: report each lane's **unique** canonical claim count and the total;
the difference is the part raised by multiple lanes together. Count canonical claims after PASS 1 deduplication, independent of disposition.
-- It measures "whether perspectives overlap"; purpose and definition in [guide.md](guide.md) "Per-lane unique claim count". Write the next step per the actual path: plan reruns review-plan; a landed PR may use the pr skill; `--no-commit-and-push` states explicitly that nothing was pushed. sync-only does not by itself force a new review round.

REJECT and FLAG are separate:
- With no REJECT, write only one line by itself `REJECT：无`; otherwise one line per item, with disposition, source, and the necessary first-party contradiction.
- FLAGGED must be the very last content of the entire output: with no FLAG, write only one line by itself `FLAGGED：无` with no text after it; otherwise give each item's complete evidence, why a decision is needed, and the options, without deciding for the developer.
```
