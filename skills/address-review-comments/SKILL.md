---
name: address-review-comments
description: Author-side adjudication and handling of plan/PR review comments. Deterministically sanitize inputs and deduplicate by root cause, then obtain first-party verification evidence for each claim (the main agent decides whether to self-verify or delegate to read-only subagents), decide APPLY / REJECT / FLAG / HOUSEKEEPING centrally, and finally remediate grouped by related root cause. Plan and PR modes land through commit-and-push by default. Use when asked to address or respond to review comments.
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

HARD RULES
- Automatically modify only claims finally adjudicated APPLY; REJECT / FLAG / HOUSEKEEPING do not change the repository.
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

========== PASS 0a: GITHUB LANE (mode == pr, and no explicit --review-file / --free-form-file) ==========

The GitHub machine reviewer is one reviewer lane of the PR; its output just lives remotely instead of in `temp/`. Fetch it and
transcribe it into a `review.md` like everyone else's, and from then on **it is no longer special**: the lane discovery glob already scans it,
and deduplication, evidence gathering, and adjudication all follow the same path.

`<pr>` = the number given by `--pr`; if not given, take the current branch's PR via `gh pr view --json number`; if that fails, STOP.
-- Both commands in this section need the PR number, which previously did not exist in INPUT at all, so the flow had no way to obtain it here.
--   The caller (merge-pr) has already resolved the number itself; passing it explicitly is more reliable than letting this skill guess again.

Run:
  python3 skills/address-review-comments/scripts/github_review_lane.py fetch --pr <pr>

`status` has four values: `head-moved` (race, fewest fields), `no-comments`, `created`, `reused`.
Only the last two provide `roundDir`, `source` (verbatim source dump + per-item timestamps), `baseRef` / `baseSha`,
and `reviewExists`; the first two lack these fields and each has its own branch below; **never let them fall into the later branches**.
-- Round identity is judged by **content** (id set + `updated_at` + body digest), and location uses `original_line`;
--   the reasons for both and the pitfalls hit are in [guide.md](guide.md) "Round identity is judged by content" and "Location uses original_line".

**One round has two lanes: inline comments (`commentCount`) and review bodies (`reviewCount`).** A body whose verdict is not 🟢
is always an outstanding comment and forms a round on its own; see [guide.md](guide.md) "The review body is itself a lane of comments".

IF `status == head-moved`:
  STOP. Someone pushed between fetching HEAD and fetching comments, so this snapshot is a race artifact: `fetch` explicitly **does not return**
  `roundIsCurrentHead`, `roundDir`, `source`, or `reviewExists`. Discard it and fetch again; do not infer a round.
  -- Why this must fail closed: [guide.md](guide.md) "head-moved must fail closed".

IF `status == no-comments`:
  This lane has no output: inline comments and outstanding review bodies are **both** zero. **IF `--github`: there is nothing to address in this run; return normally**
  (0 claims, no triage written, no ledger posted) and let the caller continue; never proceed to sanitize, where the lack of input would STOP.
  ELSE: skip this section and use the local lanes as usual.
  -- "The review is clean" is the most common case. Turning it into a STOP would halt merge-pr exactly when it should continue.

IF `roundIsCurrentHead == false` AND `cleanReviewAtHead == false`:
  STOP. Commits were pushed after the review ran: **no review has seen the tree about to be merged**.
  -- This must stop here and cannot wait for the caller to check afterwards: transcription, adjudication, and landing all happen inside this skill,
  --   and by the time merge-pr gets the return value, a stale review may already have been landed by commit-and-push.

IF `cleanReviewAtHead == true`:
  The current HEAD has a re-review **with no inline comments and a clean verdict**: the reviewer looked at this tree and had no comments.
  Inline comment groups sitting on old anchors is normal, not "nobody looked".
  -- No need to judge the verdict yourself: 🟡 / 🔵 / unrecognized bodies would already occupy the current HEAD, and this branch would be unreachable.
  -- It looks the same in `rounds` as "commits were pushed after the review ran"; the difference and consequences are in
  --   [guide.md](guide.md) "A clean re-review is not 'nobody looked'".
  IF the latest round has `triaged == true`: nothing to address in this run; handle it the same as `no-comments`.
  ELSE: address that round as usual (it is an unfinished set of comments on an old anchor).

IF `reviewExists == false`:
  Read `<roundDir>/source.md` and write `<roundDir>/review.md` per
  [github-lane-transcription](references/github-lane-transcription.md).
  That file specifies the header field values (`**分支**` uses the anchor, not head; `**判定**` is always Needs Refinement; etc.),
  the finding structure, and the fixed prior-round closure wording.
  -- **This is the agent's job, not the script's job**: you have to understand what each comment says, which dimension it falls in, and where the evidence points
  --   before you can write a finding; a script can only move text around.

IF `--github`: use **only** this lane in this run, skip legacy local lane discovery, and pass
`<roundDir>/review.md` as the **sole** `--review-file` to the sanitize step below.
-- It cannot remain a placeholder: `--github` skips legacy discovery, which is the only place that would fill in a concrete path;
--   without binding it, sanitize gets no input file and exits, and the GitHub lane never reaches PASS 1.
ELSE: this lane enters the discovery below together with the local lanes (it lives under `temp/review-pr/<slug>/github/`,
which the glob already scans).

RUN `python3 skills/address-review-comments/scripts/prepare_review_input.py \
  [--review-file <path>...] [--free-form-file <path>...] [--mode plan|pr] \
  [--label <name>...] --output <run-root>/sanitized.md`
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
- free-form requires an explicit mode; its round is `n/a`; plan/pr SCOPE_HASH is fixed at `n/a` and must not be judged review-scope-violation; the locked goal must be given explicitly by the caller/developer, STOP if it cannot be obtained, and never guess it from the diff, commits, or PR description;
- free-form lacks round provenance and SCOPE_HASH, so the closed incremental gate (d) **has no mechanical criterion available**. This is not "this round happened not to hit it"; the gate is undecidable at this entry point and must be written explicitly in triage as one line `**闸 (d)**：free-form 输入不可判`, so the gap leaves a trace instead of silently taking effect. The guardrail's gate (a′) already has a similar skip clause, and gate (d) is recorded following the same precedent;
- plan reads the full plan text and archived decisions; PR reads `<base>...HEAD`, the locked goal, the optional associated plan, and round provenance, plus the complete files and direct dependency context each claim needs;
- when plan/pr landing is needed while on master, create a feature branch first.

run root: plan/pr is `temp/address-review-comments/<slug>/<YYYYMMDD-HHmmss>/`.

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

IF mode == pr: include the reviewer test files/case hunks for APPLY claims in the allowlist; after the repair, narrowly run those tests and
directly related regressions, confirming red turns green without weakening effective assertions. When landing, commit and push the repair together with the
adopted new tests through commit-and-push, explicitly checking that new untracked files are also in the commit; never commit only the author's own production code.

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
