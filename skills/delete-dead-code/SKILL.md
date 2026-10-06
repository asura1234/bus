---
name: delete-dead-code
description: Clean up entropy on two tracks. The dead-code track fans out per module to find and delete dead code, dead branches, fake optionals, and zombie tests; the duplicate track discovers text clones and semantic duplicates across the whole scope and consolidates observably equivalent duplicates per owner set. The scope has two sources - by default the modules the current PR touches (derived mechanically by a script from base...HEAD), or --directories when the user names directories. Both tracks verify their artifacts for out-of-scope edits and self-consistency instead of relying on agent discipline; the dead-code track lands first, then the duplicate track runs on the cleaned tree. Use when the user asks to "delete dead code", "clean up dead code", "dead code", "clean up duplicate implementations", "find duplicate code", "find dead code in X", or when the pr skill invokes it before the gates.
---

Judgment principles are in [guide.md](./guide.md). Artifact structure: the dead-code track uses [references/dead-code-findings-format.md](./references/dead-code-findings-format.md),
the duplicate track uses [references/duplicate-findings-format.md](./references/duplicate-findings-format.md). This file defines only the executable flow; the
reason for every step is in the same-named section of guide.md.

**Read guide.md completely before starting**: why the two tracks are separate, the zombie rule, dead branches and fake optionals, version numbers and
compatibility branches, the bar for `CONFIRMED`, recognizing intentional duplication, the four verdicts for a duplicate group, and the consolidation
direction all live there; without it you will delete or consolidate the wrong thing.

```text
INPUT = [--base <immutable commit>] [--directories <dir>…] [--max-parallel <N, default 5>] [--track dead|duplicate|all, default all]

-- Pick one scope source: by default the modules this PR touches (`base...HEAD`); when the user names directories, pass `--directories` (guide.md
--   "When the user names directories"). Wherever `--base "<base>"` appears below, explicit-directory mode substitutes `--directories <dir>…`;
--   boundary checks, module expansion, and batching apply.
-- `--track all`: finish the whole dead-code track (including LAND) before starting the duplicate track; each track lands separately, and the order
--   cannot be swapped.

repo   = git rev-parse --show-toplevel
branch = git branch --show-current
base   = caller-provided immutable commit, otherwise origin/master
branch_slug   = branch with every character outside [A-Za-z0-9_-] replaced by `-`
artifact_root = temp/delete-dead-code/<branch_slug>

========== PREFLIGHT (must hold before each track starts) ==========

Run: python3 skills/delete-dead-code/scripts/dead_code_scope.py preflight --repo "<repo>" --base "<base>"
exit 0 = holds; exit 1 = not on a feature branch, or a unit this invocation owns has uncommitted changes (each listed); exit 2 = the scope cannot be derived.
exit != 0 → STOP.
-- Only this invocation's units are checked; other units' in-flight changes are no reason to STOP (guide.md "Why share one worktree").
-- After the dead-code track's LAND the units are clean again; a failing duplicate-track PREFLIGHT means LAND did not finish: STOP.

========== 1. SCOPE (shared by both tracks) ==========

Run: python3 skills/delete-dead-code/scripts/dead_code_scope.py scope --repo "<repo>" --base "<base>"
Exit 2 = the scope cannot be derived → STOP; never guess a scope and continue.

-- The output is the units in scope (by file count, descending). In PR mode a unit = a module (the nearest ancestor containing `AGENTS.md`, otherwise
--   the top-level subtree such as `src`); in explicit-directory mode a unit = the requested directory itself, which also owns nested modules. This is
--   the entire modifiable scope: dead code or duplicates outside it are recorded, never touched (guide.md "Why the scope must be mechanical").
-- Every unit carries `directories` (the scan list) and `excludes` (nested modules owned by others); both go into briefs verbatim.

IF --track == duplicate: skip to TRACK B.

################################################################################
TRACK A — dead-code track: discover per module, handle per module, delete only
################################################################################

========== A2. PLAN BATCHES ==========

Run: python3 skills/delete-dead-code/scripts/dead_code_scope.py batches --repo "<repo>" --base "<base>" --max-parallel "<N>"
Capture `roundCount` and `rounds`. Rounds are serial, modules within a round parallel. Never regroup them yourself.

========== A3. FAN-OUT LOOP ==========

FOR round IN rounds:
  FOR EACH module IN round:
    Dispatch ONE subagent. The brief must contain verbatim:
      - the sole allowlist: the one directory `<module>` (including same-module tests edited with the deletion)
      - that module's `directories` (scan all of them, one by one) and `excludes` from the scope output, quoted verbatim, never re-derived
      - the base commit, artifact path `<artifact_root>/dead-<module-slug>.md`, artifact header `- Track: \`dead\``, format references/dead-code-findings-format.md
      - the full text of guide.md's "Dead-code track" and "Shared" parts, naming these mandatory checks:
          * package entry points / re-export facades ("Package-level re-export facades")
          * check both directions: consumer counts find dead definitions, producer counts find dead branches and **fake optionals** ("Dead branches are checked in reverse", "Fake optionals")
          * version numbers and compatibility branches: peers delivered atomically in one binary keep no backward compatibility between them; own
            version prefixes with no v2 and constant version handshakes are deleted as dead code; guards for peers that are not delivered atomically
            (the stable endpoint contract, the socket API, installed integration assets), persisted data (including on-disk caches), and endpoints
            that may not be ours are recorded `LIKELY` + `KEPT` with the reason "needs developer ruling: version/compat"
          * this track only deletes, never consolidates: live duplicates where both copies have production callers are neither recorded nor touched;
            when only one copy has a production caller, the other is dead code: delete it and repoint tests to the live one; liveness is judged by
            production wiring only
          * when a baseline / allowlist blocks a deletion, record `HANDOFF` with the exact change to the list; never roll back a verified deletion ("Baselines and allowlists…")
      - hard constraints: no Git write of any kind; never start Bus (`./run dev`, live sessions, `just e2e`); leave no background build/test process;
        never change shared config (`Cargo.toml`, `Cargo.lock`, `justfile`, `clippy.toml`, `rust-toolchain.toml`, `.github/workflows/*`); report it instead
      - verification (only if something changed; otherwise say nothing changed): this module's tests, narrowly (Rust `just test-one <module path filter>`,
        Python `python3 -m unittest scripts.test_<name>` or `python3 -m pytest skills/<skill> -q`, Bun `bun test <test file>`); `rustfmt --check --edition 2021
        <own changed .rs files…>` (never whole-tree `cargo fmt --check`); no coverage, E2E, or whole-tree clippy/build (guide.md "How far each unit verifies itself")

  Wait for every subagent in this round. No short-interval polling.

  FOR EACH module artifact produced this round:
    Run: python3 skills/delete-dead-code/scripts/dead_code_scope.py verify --repo "<repo>" --base "<base>" --artifact "<artifact>" --unit "<module>"
    exit 0 = pass; exit 1 = out of scope or structurally self-contradictory; exit 2 = cannot verify.
    IF exit != 0
      STOP with the offending lines; the main agent reverts out-of-scope edits before deciding, never carries them into the next round.

  IF any subagent reported a blocker or a needed shared-config change
    STOP and report. Never mask it with a retry in the next round.

========== A3b. HANDOFF ROUND ==========

pending = every `path:line` whose Disposition is `HANDOFF` across artifacts (the reason names the blocking module)
IF pending is empty: skip this stage.

Split by whether the blocking module is inside the stage-1 scope:
  - outside the scope → goes unchanged into OUTPUT's "follow-up PR candidates" with the blocking module named; never widen the scope, never downgrade
    to `KEPT` (guide.md "Cross-module deletions").
  - inside the scope → pair by (module holding the symbol, blocking module):
    FOR EACH pair (at most max_parallel concurrently; further rounds beyond that):
      Dispatch ONE subagent, allowlist = the union of the two modules, only the symbols on the list. Brief as in A3, plus: no deletion beyond this HANDOFF list; run both modules' tests.
    Verify each handoff artifact as in A3, `--unit` given both modules of the pair. Whatever still cannot move after pairing stays `KEPT` and is reported.

========== A4. REVIEW (the main agent does it, never delegated) ==========

Run: python3 skills/delete-dead-code/scripts/dead_code_scope.py review --repo "<repo>" --base "<base>" --artifact <every artifact of this track…>
exit 1 = items pending review; exit 0 = no assertion rewrites, skip this stage.

FOR EACH rewritten assertion it lists:
  the artifact must carry a matching "updated to production facts" explanation. A flip without one = expectations changed to turn green. STOP.

========== A5. GATES ==========

delta = this invocation's own changes, taken per unit:
  `git status --porcelain --untracked-files=all --ignore-submodules=all -- <unit path>`
Changes outside the units belong to other owners: never inspected, never committed.

IF any agent reports a changed file outside its allowlist
  STOP "out-of-scope change" and list the files.
IF every module is CLEAN and the units of this invocation have no change
  Record "dead-code track: no confirmable dead code in this scope"; skip A6.

Feed only this track's delta files:
  `rustfmt --check --edition 2021 <delta .rs files…>`
  `cargo clippy --all-targets --locked -- -D warnings` when the delta touches Rust (crate-wide by nature; a failure outside the delta belongs to its owner)
  affected tests narrowed as in A3 (never `just test` / `just ci`, E2E, or coverage; full gates belong to gate-and-fix)

========== A6. LAND ==========

Invoke `commit-and-push` once for the dead-code delta. Whoever invokes this skill, this skill lands it (guide.md "Why it runs before gate-and-fix"):
  - stage only this invocation's delta files by explicit path: `git add -- <delta files…>` (PR mode leaves out nested modules, the same pathspec as
    preflight); before committing, check with `git diff --cached --name-only` that every file is inside a unit; never `git add -A`, `git add .`, or
    directory-level staging
  - landing is serial: only one commit-and-push runs in a worktree at a time, and when running in parallel the orchestrator holds the landing lock;
    other units' uncommitted changes on the tree stay untouched (no stash, no restore, no reset)

IF --track == dead: go to OUTPUT.

################################################################################
TRACK B — duplicate track: discover across the whole scope, handle per owner set, consolidate only
################################################################################

Re-run PREFLIGHT.

========== B1. DISCOVER (read-only, whole scope, not split per module) ==========

Run: python3 skills/delete-dead-code/scripts/dead_code_scope.py clones --repo "<repo>" --base "<base>" [--min-lines 8] [--limit 200]
Report a nonzero `truncated` or `skippedBoilerplateWindows` in OUTPUT as is; rerun with a larger `--limit` when needed; never drop candidates yourself.

`git grep` the whole scope for version traces (`[Vv][0-9]` names, `/v[0-9]+/` paths, `-v[0-9]+\.json`, version checks) and add two versions of the same thing as candidate
groups (guide.md "Version numbers and compatibility branches").

Dispatch ONE read-only subagent (semantic duplicates): the brief holds every unit and its `directories` from the scope output, and guide.md
"Discovery: across the whole scope, two ways"; it compares each unit's `AGENTS.md` responsibility section (where one exists) and public surface, finds
two places that claim the same job or hit the same socket API method / wire message / file format, reads code only for those candidates, and returns
candidate groups (member `path:line`, evidence) without any write.

The main agent merges both sources, dedups and removes containment, numbers them `G1`, `G2`…; every group has at least two members.
IF there is no candidate group: Record "duplicate track: no duplicate candidates in this scope"; go to OUTPUT.

========== B2. JUDGE (read-only) ==========

FOR EACH candidate group (at most max_parallel concurrently; further rounds beyond that):
  Dispatch ONE read-only subagent with the group's members and every judgment principle of guide.md "Duplicate track", returning:
    - verdict ∈ IDENTICAL | DRIFTED | INTENTIONAL | NOT-DUPLICATE (guide.md "Verdicts"), with an item-by-item comparison of arguments, defaults,
      failure contract, boundary clamping, and side effects; INTENTIONAL must cite the rule that requires the duplication ("compatibility with old
      peers" does not count)
    - coexisting versions not selected by an A/B feature flag converge to the newest version; when unsure, judge INTENTIONAL with "needs developer
      ruling: version/compat"
    - for IDENTICAL: the survivor (the copy production is wired to), its home, and a dependency-direction check (Rust module visibility,
      `just ui-hot-path-architecture-test` boundaries, `Cargo.toml` / `package.json` dependencies); if there is no legal home, change the verdict to INTENTIONAL
    - the owner set: every module the consolidation would change (by stage-1 unit ownership)

Summary: drop NOT-DUPLICATE; DRIFTED / INTENTIONAL go to the reported artifact; IDENTICAL with an owner outside the scope → `HANDOFF` in the reported
artifact and a follow-up PR candidate; IDENTICAL with all owners in scope → B3.

========== B3. HANDLE ==========

Merge groups whose owner sets intersect into one component (union-find), eliminating intersections before dispatch.
FOR EACH component (at most max_parallel concurrently; further rounds beyond that):
  Dispatch ONE subagent whose brief contains:
    - allowlist = the union of the component's owner set (listed verbatim), handling only the listed groups
    - each group's members, survivor, home, and verdict evidence; artifact `<artifact_root>/duplicate-<component-slug>.md`,
      artifact header `- Track: \`duplicate\``, format references/duplicate-findings-format.md
    - every principle of guide.md "Duplicate track", especially "Consolidation must preserve behavior": an assertion that flips after repointing = the
      verdict was wrong, and the group goes back to DRIFTED
    - platform-gated code may only be extracted verbatim (paste the zero-difference diff; Verification states "this target was not built")
    - hard constraints and verification as in A3, with tests covering every module in the owner set

The main agent writes the reported artifact `<artifact_root>/duplicate-reported.md` (`Outcome: REPORTED`): DRIFTED / INTENTIONAL are `KEPT` (the
reason states where it drifted or which rule), groups with owners outside the scope are `HANDOFF`.

========== B4. VERIFY ==========

FOR EACH duplicate artifact (including the reported artifact):
  Run `dead_code_scope.py verify --repo "<repo>" --base "<base>" --artifact "<artifact>" --unit <owner set…>`
  (the reported artifact covers the whole scope; no `--unit`)
  IF exit != 0: STOP with the offending lines; the main agent reverts out-of-scope edits before deciding.

IF any component agent reported a blocker or a needed shared-config change: STOP and report.

========== B5. REVIEW (the main agent does it, never delegated; guide.md "The main agent must review consolidations itself") ==========

Run: python3 skills/delete-dead-code/scripts/dead_code_scope.py review --repo "<repo>" --base "<base>" --artifact <every artifact of this track…>
exit 1 = items pending review (the normal case); exit 0 = no consolidation and no assertion rewrite, skip this stage.

FOR EACH CONSOLIDATED anchor it lists:
  a. Read the diff and identify the surviving copy (CANONICAL) and the deleted copy.
  b. Read back from production call sites and confirm the surviving copy is the behavior production was running; wiring is the only criterion, never
     exports, docstrings, what tests point at, or `AGENTS.md`.
  c. Compare the observable differences of both copies: arguments, defaults, failure contract, boundary clamping, side effects.
  d. Confirm no forbidden dependency edge was added and no broad shared module was created.
  IF the surviving behavior != what production ran, or the dependency direction is illegal
    STOP. Send the group back: redo it from production facts or re-judge it DRIFTED / INTENTIONAL; write the behavior difference into the reported
    list as a product question.

FOR EACH rewritten assertion it lists:
  an assertion that was rewritten (rather than deleted wholesale with the deleted copy, or repointed unchanged) must be explained in the artifact;
  otherwise STOP.

========== B6. GATES & LAND ==========

Same as A5: take the delta per unit; STOP on self-reported out-of-scope changes; when no component changed anything, record "duplicate track: no
consolidatable duplicates" and skip landing; otherwise run the rustfmt / clippy / narrow tests the combined delta needs.
Invoke `commit-and-push` once for the duplication delta (separate from the dead-code commit), with the A6 rules.

========== OUTPUT ==========

Dead-code track: module and round counts; each module's Outcome (CLEAN / ACTED / REPORTED) and artifact path; deletion summary (including dead copies
and fake optionals); every `LIKELY` (not deleted) and `KEPT` (with reason); unhandled out-of-scope dead code and HANDOFFs blocked outside the scope
(follow-up PR candidates); this track's commit.

Duplicate track: clone candidate count (including `truncated` / `skippedBoilerplateWindows`), semantic candidate count, group count; each group's
verdict, survivor, and home; consolidation summary and each component's artifact path; behavior differences of DRIFTED groups (product questions),
rules cited by INTENTIONAL groups; groups with owners outside the scope (follow-up PR candidates); this track's commit.

Both tracks: the developer-ruling list (items whose reason is "needs developer ruling: version/compat", plus compatibility branches recorded as
HANDOFF across modules, naming which producer changes first); unrun gates (coverage, E2E, the Windows target, other unbuilt platform targets) listed honestly.
```
