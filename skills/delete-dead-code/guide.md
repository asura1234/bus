# Delete-Dead-Code Guide

`SKILL.md` defines the execution order; this file defines judgment. This skill has two tracks with different criteria,
run in order:

- **Dead-code track**: one core criterion: **prove it has no consumer, then delete it**. It only deletes, never
  consolidates.
- **Duplicate track**: the core criterion is **prove the copies are observably equivalent in production, then converge
  onto the copy production is wired to**. It only consolidates, never deletes standalone dead code.

## Why the two tracks are separate

They look for things of different shapes, so they must be sliced differently:

- **Dead code is a property of a single symbol**: "does anyone use this definition". An agent that owns a module scans
  its module and searches the whole repository for consumers, and can answer completely. Slicing per module loses
  nothing.
- **Duplication is a property of a pair of code spans**: "these two spans do the same thing". Sliced per module, each
  agent only sees pairs inside its module; yet the most valuable ones are exactly the **cross-module** duplicates, two
  packages that each reinvented the same thing (observed: the engine carried its own `/v1/exports` client, and each
  persistence export target carried its own XML helper). In the first full cleanup fanned out per module, the merged
  duplicates were almost all small helpers inside one module, and the cross-module ones appeared only as side notes.

So: dead code is **discovered per module and handled per module**; duplicates are **discovered across the whole scope
and handled per owner set**.

The actions must be separate too. Deletion does not change behavior; consolidation may, and when its direction is
reversed the gates are guaranteed green (see "Duplicate track: the main agent must review consolidations itself"). With
both actions in one artifact and one commit, a consolidation gets waved through as a deletion during review. Artifacts
therefore carry a `Track`, `verify` rejects kinds and dispositions from the other track, and the two tracks land
separately.

**The dead-code track runs first and lands first**: a copy that has no production caller at all is dead code; delete
it, and it never enters the consolidation judgment. Only after that cleanup does the duplicate track see nothing but
real duplicates where "both copies are alive".

## Why the scope must be mechanical

The one truly dangerous failure of this skill is not a missed deletion but deleting outside the PR scope. It was
observed once: the developer said "this PR", the agent was holding whole-repository audit conclusions, and it cleaned
`cli/`, `scripts/`, and packaging along the way. Not disobedience: the conclusions in context were more persuasive than
the reminder.

So the scope is derived by `dead_code_scope.py` from `base...HEAD`, and artifacts are checked for out-of-scope edits,
instead of relying on agent discipline. Each top-level subtree such as `src/` or `scripts/` is a unit; loose top-level
files form a separate unit. No separate layering table is kept.

Even confirmed dead code in a module the PR did not touch is **not in this scope**: record it and raise it separately,
never delete it along the way.

The same mechanical output also gives every module a **directory map** (`directories`). It addresses a failure in the
other direction: the scope check stops outward drift but not inward leaks. "Scan this module" is an instruction with no
end, and an agent that has looked at a few suspicious-looking directories will feel done. A directory list gives the
scan an enumerable completion condition: walk every directory, not every hunch. The list comes from `git ls-files`, so
`target/`, `node_modules/`, and `dist/` do not eat the scan budget.

## When the user names directories

`--directories` swaps the scope source, not the scope discipline. When the user says "find dead code in `src/client`
and `src/server`", those trees are the entire scope: even confirmed dead code outside them is only recorded, never
deleted, exactly as in PR mode.

The two modes differ in two ways, both essential:

1. **How many files to look at**: PR mode looks only at the modules owning changed files; explicit mode looks at every
   tracked file in the whole tree. The scan must walk the directory list one by one, never carry over the habit of
   "look only at what the PR changed".
2. **What a unit is**: in PR mode a unit is the top-level subtree, or the loose top-level files; in explicit mode a
   unit is the requested directory itself, including its subdirectories.

The second is decided by "can it act", not by "how large is the scan". Observed: the canonical home of real dead code is
usually a **sibling module**: base64 in `tools` wanted to move to `codecs`, the Windows helper to `process`,
`canSplitClip` in `core` to converge into `model`. An agent owning one leaf module could only record `HANDOFF` for all of
them, so in that round `native/media/src/tools` acted on none of its 7 findings; the agent owning the whole of
`native/media` just did them.

The cost is larger units and more context pressure on a single agent. Granularity is the caller's choice: pass more
subdirectories to slice finer.

A scope that cannot be derived (base does not exist, empty diff, a directory that does not exist or has no tracked
files, a directory outside the repository or equal to the repository root, requested directories that contain each
other) is always a hard failure. When a parent and a child directory are both units, one file has two owners and the
premise of sharing a worktree collapses; overlap is judged on the normalized repository-relative path, so relative,
absolute, and `./` spellings cannot bypass it. A misspelled directory name would silently scan nothing and return in the
shape of "this is clean", and nobody could tell from the output that the scan never happened.

## Why share one worktree

Units are pairwise disjoint: every unit has exactly one owner, and an owner writes only its own unit. With
non-overlapping write sets, several invocations can run in parallel in **the same worktree**; preflight (`preflight`),
verification (`verify`), assertion review (`review`), and landing's staging therefore look only at the units this
invocation owns, never at the whole tree. Other units' in-flight changes can only belong to other owners: they are no
reason to stop, and they never enter this invocation's commit.

Isolated worktrees are needed only when **write sets really overlap** (two invocations must change the same files). In
that case say up front how many will be created, and close them one by one afterwards. Making "the whole tree is clean"
a precondition and then handing every agent its own worktree to satisfy it shifts the cost of an over-wide check onto
orchestration. One observed full cleanup over 8 slot worktrees paid every cost to that isolation layer itself:

- **Bootstrap failures**: the first dependency preparation in a fresh worktree could not get the dependency archive, and
  parallel preparation trampled the shared download cache; slots could only warm up serially, and several never came up.
- **Stale slots**: before every unit a slot had to move to the integration branch's latest commit; the reset step
  sometimes failed, the slot stayed on an old commit, and every unit queued behind it was blocked.
- **Patch shuttling**: changes made in a slot and landed in the main worktree could only travel as exported patches and
  `git apply --3way`; conflicts and rollbacks each needed their own procedure.
- **Overreaching overwrite**: one directory-level `git add -A` committed other changes on the tree along with it,
  silently overwriting 9 files of another change that had already landed.

The last one is exactly the discipline a shared worktree must keep: **landing is serial** (only one commit and push at a
time), **only this unit's paths are staged** (explicit paths, checking the staged list before committing), and other
people's changes on the tree stay untouched: no stash, no restore, no reset.

The cost is that out-of-bounds writes can no longer be spotted as "out-of-scope files in the whole-tree diff": a shared
tree has out-of-scope changes anyway. So out-of-bounds writes are judged from the agent's self-reported change list,
`verify` restricts artifact anchors to the artifact's own unit (`--unit`), and the staged-list check before landing is
the last line.

The unit-clean check carries `--ignore-submodules=all`. That is not a relaxation but an exclusion of unrelated state: a
submodule gitlink carries its own HEAD and worktree, and this skill never edits submodules.

# Dead-code track

## Package-level re-export facades

**This is a class that was observed to slip through.** In one dogfood round an agent checked consumers symbol by symbol
yet missed `cli/desktop_pipeline/__init__.py` (100 lines) and `cli/devtool/__init__.py` (18 lines) entirely: two pure
re-export facades that nobody in the repository imported from.

It slipped because the wrong question was asked: asking "does anyone use this symbol" per symbol, the names in a facade
**do exist and are alive elsewhere**, so each one looks alive. The question to ask is a different one: **"does anyone
import through this facade"**.

- Rust: `pub use` re-exports in `mod.rs` / `lib.rs` / `main.rs`. The criterion is whether consumers name the facade path
  (`crate::<module>::<Symbol>`) or the deep path (`crate::<module>::<submodule>::<Symbol>`); when only the deep path is
  used, the re-export is dead.
- Python: `from .x import Y` + `__all__` in `__init__.py`. The criterion is `git grep "from <pkg> import"` plus
  `<pkg>.<symbol>` attribute access; when there is only `from <pkg>.<submodule> import`, the facade is dead.
- TypeScript: an `index.ts` that only does `export * from './x'`. Same criterion: do consumers import the package name or
  the deep path.
- The same shape includes dynamic re-export loops like `globals().setdefault(...)`.

**Same shape, opposite liveness is the biggest trap here**: in the same round another agent correctly deleted a dead
`globals().setdefault` loop while keeping a live facade of identical shape that 8 modules depended on. So the criterion
is always the consumer query, never the look.

When scanning each module, treat "this module's package entry points / re-export files" as a mandatory item instead of
waiting for them to surface during the symbol walk: they will not.

## The zombie rule

A dead function and "the dead test that only tests it" pin each other alive. **A test reference is not evidence of
life**: it is just the other half of the dead code. When production has no caller left and only tests call it, delete
both.

The exception is a **real injection seam**: the production code itself is alive, and the test only injects through the
seam. Keep these. Tell them apart by the production side:

| | Production side | Verdict |
| --- | --- | --- |
| Zombie pair | No caller at all | Delete both |
| Real injection seam | Real callers; the test only injects a double | Keep |

Typical shapes of a real injection seam: a fault flag fed by a test hook, an observation entry named `*_for_testing`, a
replaceable clock source, a `panic_for_boundary_test` exported specifically for a boundary test.

**Observation seams need one more layer of thought**: a `channel_count()` read only by tests looks like it fits the
zombie rule, but deleting it may also delete the only assertion of the leak invariant "`disconnect` really closed the
channel". When the production method is alive and only the observation entry is test-only, lean toward keeping it and
reporting it; never silently weaken the test.

## Baselines and allowlists pin dead code (the second shape of the zombie rule)

The zombie rule is about dead functions and dead tests pinning each other alive. There is a subtler shape: **a derived
file registers this code by position, so deleting it breaks that file, and that file is usually outside everyone's
allowlist.**

Observed: five helpers in `process_rpc/process_host_http.cpp` had no caller on **any** platform (not platform-gated live
code, truly dead). The agent deleted them, the build passed, and then the whole thing was rolled back: a native-hygiene
allowlist JSON pinned four NOLINT exemptions in the same file by `path:line`, deleting 7 lines shifted all four keys, and
native hygiene immediately reported 4×`unused-allowlist-entry` + 4×`suppression-not-allowlisted`. That list was
repository-level shared config, owned by no module agent.

The result: **this dead code structurally can never be deleted**. Every cleanup round rediscovers it, deletes it again,
hits the same wall, and rolls back again. Nobody did anything wrong, and it stays on the tree forever.

These "pins" are not limited to one language or one linter. Any **derived file that references code by path or line
number** counts:

- suppression lists and baselines of lint / static checks (exemptions registered per `path:line` or per file)
- coverage exemption lists
- size / line-count baselines
- snapshots, golden files, generated-artifact inventories (Bus's agent detection manifest checked by
  `scripts/agent_detection_manifest_check.py`, the configuration reference checked by `scripts/config_reference_check.py`,
  the socket API schema `src/protocol/api/schema/bus-api.schema.json`), CODEOWNERS-style path tables

Rules:

1. **Baselines and allowlists are not consumers.** Same criterion as "prose is not a consumer" and "a test reference is
   not evidence of life": they record "this code once existed and was let through", not "someone uses it". Being pinned
   is no evidence of life.
2. **Updating the pin is part of the deletion**, not a precondition for it. When you find a pin, never roll back a
   verified deletion, and never downgrade it to `KEPT`: `KEPT` means "investigated, it should stay", and the conclusion
   here is the opposite.
3. **Handle it as `HANDOFF`**, with the blocking module being the module that holds the list (in Bus usually `scripts`,
   `.github`, or the repository root), and write in the reason exactly how to change it: which keys, how many lines to
   shift, or delete the whole entry. The paired agent then gets a mechanical operation, not a fresh investigation.
4. **Line-number coupling is invisible**: the code is in one file, the pin in another, the two in different modules, and
   `git grep` on the symbol name cannot find it; the list holds only a path and numbers. So every time you are about to
   delete **whole lines**, also ask "does any baseline record this file by line number", instead of waiting for a build
   failure to tell you.

⚠️ Beware the reverse too: the pin itself may be what is dead. An allowlist entry pointing at a symbol that no longer
exists, or at a line that has long been compliant, is residue on the list side; it too can only be deleted by the module
that holds the list. Both cases go through `HANDOFF`, and the reason states which one it is.

## The bar for confirmation

`CONFIRMED` requires a whole-repository grep and an understanding of the counterexamples below; otherwise it can only be
`LIKELY`, and `LIKELY` is never deleted.

- **Reachable through strings**: socket API method names (`src/protocol/api/schema`, `bus-api.schema.json`), wire-protocol message
  names, CLI subcommand and flag names, config keys (including `serde` renames), integration asset names and plugin
  discovery, `include_str!` / `include_bytes!` assets, macro-generated names, `justfile` recipes and command lines in CI
  workflow YAML, FFI / `#[no_mangle]` exports, dynamic `import()` in TypeScript. Search the bare name, the quoted name, and
  case-converted names.
- **Stale build artifacts**: `grep -r` ignores `.gitignore` and hits old outputs in `target/`, `out/`, `dist/`, `build/`,
  displaying a dead symbol as "still used"; the consequence is a **silently missed deletion**. Use `git grep`, or
  explicitly exclude these directories.
- **False friends with the same name**: a Rust local variable or a TypeScript identifier in an integration asset may share
  a name with the symbol. Check that the hit is the same thing.
- **Platform gating**: code under `#[cfg(unix)]`, `#[cfg(windows)]`, `#[cfg(target_os = …)]`, or `#[cfg(test)]` /
  feature gates has no caller in another build configuration, but it is not dead code. Confirm the real build target set
  first (CI builds Windows; `just windows-lint` checks the Windows target from Unix).
- **Prose is not a consumer**: a symbol mentioned in `docs/**/*.md` or a skill does not make it alive.
  Delete the symbol and fix the prose with it.

## Dead branches are checked in reverse: count producers, not consumers

**This is a whole class that was observed to slip through.** In one dogfood round an agent counted symbol consumers one
by one across 196 files and still missed, wholesale, what manual handling found: those symbols **all had consumers**;
what was dead was the branch, not the definition.

Consumer counts can only answer "is this definition still referenced". A dead branch has the opposite shape: **the symbol
is alive, but the value being compared has no producer at all**. The check must switch direction; for every branch that
dispatches on a value, ask "who **can** produce this value?":

- **Collapsed union types**: `resourceKind === 'stream'` is always false because `RenderableResourceIdentity.kind` had
  already converged upstream to the single literal `'frame'`. Check: take the compared literal back to the **type
  definition** and see whether that member still exists (Rust: an enum variant that is matched but never constructed).
- **Event names / status strings with zero emitters**: `viewer-mse-media-appended` was mentioned only by the harness and
  its own tests; nothing in production emitted it. Check: `git grep` twice, counting the emit side and the handle side
  separately; a handle side alone is a dead branch (often also a zombie loop: the harness produces and tests itself).
- **Constant arguments**: an optional parameter that **every** call site omits (or passes the same constant for) makes the
  second path it guards unreachable. Check: enumerate every call site and look at the actual argument, not at whether the
  parameter is used. The reverse, where every call site **passes** it, is a "fake optional"; see the next section.
- **Unreachable enum members**: a member of a union / enum with no construction site.
- **Branches kept for old peers**: paths guarded by "old binary / old shape / legacy / compat" comments, protocol version
  checks, dual reads. The question is whether the **current** peer still produces that shape; if not, it is a dead branch,
  and the reason "both ends may be out of sync during development" does not hold (see "Version numbers and compatibility
  branches"). When the current peer **still** produces the old shape, change the producer first through `HANDOFF`, not
  `KEPT`.

**Why it deserves its own section**: this residue is the typical product of large migrations (libtv-desktop's MSE →
shared-texture migration produced the first four kinds above), and it is completely immune to consumer counts: every
symbol counts as alive. It is also the most dangerous: a branch you can walk into and never out of is a diagnostic risk,
because readers believe that path still works.

Deleting a dead branch often takes a whole chain with it: the predicate, the synthetic fixtures, the cases that only test
it. Delete them together under the zombie rule.

## Fake optionals: optionals every production call site passes

**Definition**: a field, parameter, or option is optional in its type (`Option<T>`, `?:`, `| undefined`, `| null`, a
parameter with a default), but **every production construction site provides a value**. The default path it guards
(`unwrap_or` / `unwrap_or_default` / `??` fallbacks, `?.` short circuits, `None` / `=== undefined` branches, parameter
defaults) has no producer at all, so it is a dead branch; the "may be absent" in the type is a lie.

**Why it is common**: it is a typical residue of incremental agent development. Marking a new interface field optional
means no existing call site has to change and the type check passes immediately; then a fallback default keeps
downstream quiet. Each step is locally reasonable; together they make a field that is never absent but is handled as if
it could be, and readers believe the default path really runs in production.

**Check** (count producers, not consumers):

1. Enumerate **every production construction site**: call sites, struct / object literals, factory return values,
   spread / `..Default::default()` assembly, conversion impls, `satisfies` / type assertions. If any site omits it
   conditionally (`cond ? { x } : {}`, `Partial<>` passthrough, an upstream that is itself optional), it is not a fake
   optional.
2. **Tests are not producers.** Only tests omitting it is no reason to keep it optional; this is the most common
   misjudgment in this section.
3. Confirm one by one that the passed value's type is itself non-optional; if the upstream is optional too, keep walking
   up the chain to the real constructor.

**Disposition**:

- Make it required (or drop the `Option` / `| null`), delete fallback expressions, short circuits, and parameter
  defaults, and with them the comments and constants that only served the default path. The default path never ran in
  production, so **this is not a behavior change** and needs no product ruling.
- Tests that rely on omission: **never keep the optional for tests**. Provide one construction helper in the test tree
  (for example `router_options(overrides)`) that supplies explicit values centrally, and switch the cases to it; never
  inline a copy of the default into dozens of cases, which only moves the same fake default into the tests.
- A case that specifically asserts default-path behavior is a zombie test and is deleted under the zombie rule; if it
  still holds a unique assertion, merge that into a surviving test first.
- "The exported API gets stricter" and "many tests must change" are **not** reasons for a developer ruling: in-repo
  callers are fixed in the same pass, and cross-module cases are handled in pairs via HANDOFF. Only the real optionals
  below stay.

**Real optionals, do not touch**:

- **Boundary input that is optional in the current contract**: fields that the **current contract** allows to be absent
  in socket API requests, config files, CLI input, JSON parsing, or cross-language schemas; optional plus boundary
  validation is the correct model there. The reason can only be the current contract, **never "an older peer might not
  send it"**: peers inside the same binary are delivered atomically, and compatibility of remote services belongs to the
  service (see "Version numbers and compatibility branches"). An optional kept only for old peers is a fake optional;
  guards for peers that are not delivered atomically are judged separately in that section.
- **Out-of-scope or out-of-repository consumers**: when vendored code or a module outside this scope also constructs it,
  record `HANDOFF`; never tighten one side alone.
- **Absence carries meaning**: among the production construction sites some pass it and some do not, and absence
  represents a real state (for example "nothing selected").

**Hand to the developer, never keep silently**: optional fields in persisted data (session snapshots under
`src/persist`, the user config file, on-disk caches and formats). Bus is released, and changing their shape affects old
data on installed users' disks and needs a migration decision; this skill does not cross that line itself, so record
`LIKELY` + `KEPT` with the reason "needs developer ruling: version/compat", into OUTPUT's developer-ruling list.

## Cross-module deletions (HANDOFF)

Module granularity is right for "scan depth" and insufficient for "acting". Real dead code mostly crosses boundaries:
the symbol is in A, its only caller or test in B. In the first dogfood round all five modules hit this; one actually
deleted three write-only fields, the build failed on another module's tests, and it had to roll back.

The disposition is not "write it down and move on" but `HANDOFF` naming the blocking module: after the fan-out the main
agent groups HANDOFFs of the same module pair and dispatches one agent that **owns both modules**, handling only the
symbols on the list.

Never downgrade it to `KEPT` just because you cannot move it: `KEPT` means "investigated, it should stay", and HANDOFF
means "it should be deleted, but two owners must move together". Conflating them leaves real dead code on the tree
forever.

When the blocking module is **outside this scope**, the pairing stage cannot absorb it: it goes unchanged into
"follow-up PR candidates" with the blocking module named, neither widening the scope (exactly the overreach this skill
must prevent most) nor downgrading to `KEPT`. Observed: all three HANDOFFs of `native/media/src/process` pointed at
`native/media/src/tools` and `native/media/tests`, and that PR had not changed a single byte of those two modules.

## Delete or report

- **Delete**: no consumer confirmed, and no other implementation carries its responsibility.
- **A "duplicate" with one live copy and one dead copy is dead code**: when only one of two implementations has a
  production caller, the other (often the exported one, with a docstring, that tests point at) is dead code; this track
  deletes it directly and repoints tests to the live one (see "Tests follow the code"). **Which copy is alive is judged
  by wiring only**; for why, see the `createSessionScrubPreview` incident in "Duplicate track: consolidation must
  preserve behavior".
- **Live duplicates where both copies have production callers do not belong to this track**: not recorded, not
  touched. The duplicate track rediscovers them across the whole scope; handling them here would only turn them into a
  behavior change that skipped consolidation review.
- **Report without acting**: cases that need a product decision, and any case where "fixing it right crosses owners"
  and still cannot move after pairing. Half done is worse than not done. Cross-language generated contracts are not in
  this category: change the contract source, regenerate, and `HANDOFF` in pairs (see "Version numbers and compatibility
  branches").

## Tests follow the code

When deleting a production implementation, delete the tests that only test it. But **first check whether the test also
asserts something else**:

- When the test holds a unique assertion (object identity, inputs not mutated, cross-platform consistency markers), merge
  it into a surviving test before deleting.
- When the test points at a duplicate implementation while production runs another, repoint the test to the live one
  instead of deleting it: it covers real behavior.
- An assertion that flips after repointing means the two implementations had drifted apart: that is a **finding**;
  update the assertion to production facts and report it; never change expectations just to turn green.

# Duplicate track

## Discovery: across the whole scope, two ways

Duplicate-track discovery is not sliced per module; the corpus is **the whole scope** (in PR mode the complete directory
trees of every touched module, in explicit-directory mode every requested directory). The two ways complement each
other, and each is blind to the other's targets:

1. **Text clones**: `dead_code_scope.py clones` finds spans across the whole scope that are line-for-line identical once
   layout is removed (skipping blank, comment, import/include/`use`, and bracket-only lines; by default at least 8
   effective lines and production code only). It is cheap, deterministic, and naturally cross-module. It only answers
   "these two look the same"; whether it is intentional, whether it can converge, and where to, are still unanswered. A
   window that occurs too many times counts as boilerplate (generated code, protocol tables) and is not paired; the
   output states how many were skipped and how many truncated.
2. **Responsibility comparison (semantic duplicates)**: the same job in different code, which clone detection cannot find
   a single line of (observed: the engine's `export-wire.ts` and media-tools' `createCompositeMediaToolsClient` both hit
   `/v1/exports`). Derive each unit's **responsibilities** from its code and public surface (`pub` items in
   `mod.rs` / `lib.rs`, socket API
   methods, CLI subcommands, re-exports), find two places that claim the same job, or hit the same socket API method /
   wire message / file format, and only then read code for those candidates.

Candidates from both ways are merged and numbered as duplicate groups (`G1`, `G2`…), each with at least two members.

## Intentional duplication is not residue

Multiple copies of one implementation are sometimes **required** by a boundary, not a copy that slipped through:

- A dependency rule forbids A from depending on B, so A must carry its own copy (the UI hot-path architecture boundaries
  enforced by `just ui-hot-path-architecture-test`; bundled integration assets under `src/integration/assets` and the
  `workers/` package that ship outside the crate and cannot import it).
- `#[cfg(unix)]` and `#[cfg(windows)]` implementations with the same name and responsibility are **platform pairs**, not
  duplicates; consolidation only happens inside one platform.
- An independent implementation in tests is a **judging oracle**: asserting production with production makes the
  assertion a tautology.
- Two seemingly identical functions with different failure contracts (one errors, one returns empty) are two functions.

Criterion: **first find the rule that forbids the dependency**. If you find it, it is design: record it and skip; only if
you cannot is it residue.

## Verdicts: each candidate group has only four outcomes

- **`IDENTICAL` (can converge)**: the members are **observably equivalent** in production: byte-identical, or confirmed
  identical after comparing arguments, defaults, failure contract, boundary clamping, and side effects item by item. Only
  this verdict may converge.
- **`DRIFTED` (report)**: looks alike, but the behavior has drifted. Converging would mean ruling which copy wins, which
  is a **product change**: it goes into the report list, not into the cleanup. The only exception: the drifted part
  happens to be a dead branch (no producer), proven first by the dead-code track's criteria; once removed and equivalent,
  it counts as `IDENTICAL`.
- **`INTENTIONAL` (keep)**: you can find the rule that requires this duplication (see the previous section). `KEPT`, with
  that rule as the reason. "Compatibility with old peers" is **not** such a rule (see "Version numbers and compatibility
  branches").
- **Coexisting versions (`v1` / `v2`…)**: an old and a new version of the same thing are not `DRIFTED`; the drift is the
  version upgrade itself. Unless an A/B feature flag selects between them, converge to the newest version and delete the
  old one; behavior specific to the old version is not a product fact to preserve. Only when the version of persisted
  user data is uncertain is it `KEPT`, with the reason "needs developer ruling: version/compat".
- **Not a duplicate**: looks alike, different responsibility (for example two identical loops computing different
  things). It does not enter the artifact.

For an `IDENTICAL` group two more things must be settled:

- **The survivor (`CANONICAL`)**: the copy production really calls today; when both are called and equivalent, pick the
  one located where "every user is allowed to depend on it".
- **Home and dependency direction**: after consolidation every user depends on the survivor. First check the repository's
  dependency boundaries (Rust module visibility and `pub(crate)` reach, the UI hot-path architecture test, `Cargo.toml` /
  `package.json` dependency declarations): if some user may not depend on the survivor's module, move it to a lower module
  both sides may depend on; if no such place exists, the group is `INTENTIONAL` (the boundary requires a copy on each
  side). **A consolidation must not add a forbidden dependency edge, and must not create a broad shared module just to
  converge.**

The modules a group's members fall into are that group's **owner set**. Groups whose owner sets intersect must go to the
same agent: two agents writing one file is an orchestration error. A group with an owner outside the scope is recorded
`HANDOFF` and becomes a follow-up PR candidate.

## Duplication in platform-gated code: verbatim extraction is allowed

"Keep everything this machine cannot build" misses duplication that really exists. Observed once:
`platform/windows/frame_encoder.cpp` and `platform/windows/cdp_pipe_client.cpp` each had a byte-identical `closeHandle` /
`wide` / `quote` (about 41 lines ×2), and `src/process/external_tool_process.cpp` had a third copy. Manual handling in the
same round merged them, while the skill recorded all of them as `KEPT` because "Windows is unverifiable". **That is not
caution; it excludes a class of confirmed duplication from the skill's capability.**

The dividing line is **the nature of the change**, not the platform:

- **Verbatim extraction (allowed)**: move N byte-identical implementations into one shared module and have the call sites
  use it. This is mechanically provable: `diff` between the extracted function body and each original must show **zero
  difference**, and the artifact pastes that diff as evidence. The only remaining risk is "are the module path and the
  `cfg` gate / build registration right", which is visible on any platform.
- **Rewrites that need judgment (not allowed)**: several copies have drifted and someone must rule which behavior wins;
  or the signature or error handling changes along the way. On a platform that cannot be built these are always
  `LIKELY` + `KEPT`, with the reason stating where they drifted.

After a verbatim extraction, the artifact's `## Verification` **must** explicitly state "<platform> target not built"
(for Windows, `just windows-lint` from Unix is the closest check), and the main agent reports it verbatim in the summary.
Unrun gates are listed honestly, rather than skipping the work because they cannot run.

Correctness-sensitive helpers like `quote` command-line escaping show no compile-time sign when three copies drift; that
is exactly why they should converge rather than stay.

## Consolidation must preserve behavior

**The canonical implementation = the copy production really calls today.** Not the exported one, not the one with a
docstring, and not the one tests point at. It went wrong once: two
`createSessionScrubPreview` copies; the exported one had a full docstring and three tests pointing at it,
while production wiring went through a module-private copy in the same file. The
agent picked the exported one because it "looked more official", so **a cleanup action changed production behavior**
(exact settle started keeping the visible frame).

When consolidating, repoint users to the survivor and delete the other copies; tests that only tested a deleted copy are
repointed to the survivor rather than deleted, because they cover the same real behavior. An assertion that flips after
repointing means the two copies had in fact drifted: the verdict was wrong, the group should be `DRIFTED`, send it back,
never change expectations just to turn green. Neither prose nor tests are the source of truth about behavior; **wiring
is**: read back from the real call sites and see which one production actually constructs / calls.

## The main agent must review consolidations itself

The default assumption of a fan-out, "whatever the subagent reports is so", holds for deletion and does **not** hold for
**consolidation**.

This skill can cause exactly one kind of product incident: a reversed consolidation. It has three properties that let it
slip past every existing defense:

1. **The gates are guaranteed green.** The tests are pointed at the surviving copy along with the consolidation, so there
   is no failure signal at all.
2. **The subagent's self-report is not evidence.** It is the one that made the wrong judgment, and its rationale reads
   just as coherent.
3. **The boundary check cannot see it.** The change stays entirely inside its own owner set, and `verify` passes it all
   the way.

So REVIEW is a fixed duty of the main agent, not an optional spot check, and it cannot be delegated to another subagent.
The script's `review` subcommand enumerates both mandatory item kinds (every `CONSOLIDATED` anchor, and test assertions
that were **rewritten** rather than deleted) and exits nonzero while items are pending: without finishing this stage you
cannot reach landing.

The review accepts only one thing: **reading back from production call sites**. Exported or not, docstring or not, what
tests point at: in practice all three pointed at the wrong copy.

# Shared

## Version numbers and compatibility branches

**Premise**: Bus is a released binary (a Herdr fork) whose peers come in three kinds, each with different compatibility
responsibility.

- **Peers inside the same binary are delivered atomically**: Rust modules of the crate, the embedded integration assets
  it writes out, and the client and server halves of one install upgrade together; there is no "new module paired with an
  old module". Schema changes between them are made on both ends at once and fail fast, with no version number, dual read,
  migration, or compatibility branch; endpoints between repository-owned tools (`scripts/`) and the binary work the same
  way.
- **Peers that are not delivered atomically**: the stable endpoint contract that client-owned shells and remote endpoints
  negotiate (`src/protocol/endpoint.rs`), the socket API used by external tools and agents, and integration assets already
  installed into users' agent configurations may run against other released versions; branches guarded by that
  negotiation can still have producers. Judging them dead first requires proving that no supported peer version below the
  negotiated floor still needs them; if that cannot be proven, record `LIKELY` + `KEPT` with the reason "needs developer
  ruling: version/compat".
- **Compatibility of remote services belongs to the service.** The client is only responsible for **forward tolerance**
  (ignore unknown fields and unknown enum values), and never writes branches for an **older** service or peer shape.

So every version/compatibility trace (`v1` / `v2` / `v3` names or paths (`fooV2`, `/v2/`, `*-v1.json`), protocol or
schema version checks, dual reads / dual writes, "old binary / old shape / legacy / compat / old peer" comments guarding
a branch) is handled as follows:

- **Duplicate** (two versions of the same thing coexist) → duplicate track: converge to the newest version, delete the
  old one.
- **Dead branch** (a compatibility path kept for an old shape the current peer will never produce again) → dead-code
  track: delete it.
- **Exceptions**: when an A/B feature flag selects the version, both are alive; do not touch them. Guards for peers that
  are not delivered atomically are judged by the premise above.

An observed libtv-desktop example: media-tools' loopback transport kept a branch for "old shape: 503 with an empty body",
reasoning that "native and this package may be out of sync during development", together with a test "an old native's
empty-body 503 is still recognized". Peers in the same package are delivered atomically, so the reasoning does not hold.
But at the time it was **not yet a dead branch**: a native connection-layer rejection (`enqueueConnection` failing) still
sent an empty-body 503. The order of handling was to change the **producer** first (native sends the overloaded envelope
here too); only then did the client branch become dead and get deleted. That was a cross-module `HANDOFF`
(native/media → media-tools), **not** `KEPT`.

**Without a v2 or v3, a v1 means nothing.** A lone version marker on our own protocol is equally dead, not "pending a
ruling":

- **Path version prefixes**: `/v1/…` on our own protocol is just noise when no `/v2/` exists. Remove the `/v1`, renaming
  producer and consumer together (atomically delivered peers in one binary: change both ends directly).
- **Constant version handshakes**: fields, constants, and checks whose only legal value is a single number
  (`protocolVersion: 1`, `schema_version != 1 → reject`). They exist only for "peer version drift", and atomically
  delivered peers do not drift: delete the handshake field, constant, check, fixture, and the cases that only test that
  check together.
- When the rename crosses languages or modules, handle it in pairs via `HANDOFF`; for generated code change the contract
  source and regenerate, never hand-edit generated output.

**Only these go to the developer, never kept silently "to be safe"**:

- **Persisted user data**: the schema version of on-disk formats (session snapshots under `src/persist`, the user config
  file) is **not** a lone version marker: released users' disks hold old shapes, and the reading side relies on it to
  decide between migrating and discarding; never delete it as dead code. Shape changes go to a developer ruling; the
  invalidation keys of on-disk caches (for example a cache that misses by `schemaVersion`) likewise.
- **Uncertain whether the peer is ours**: when you cannot tell whether an endpoint is a peer in the same binary or a
  remote service (for example a catalog fetched from a remote source with its own `schema_version`).
- **Guards for peers that are not delivered atomically**: see the premise above.

These are recorded `LIKELY` + `KEPT` with the reason "needs developer ruling: version/compat", into OUTPUT's
developer-ruling list. A remote service's own URL version (for example a provider's `/v1/` API) is not our protocol:
not recorded, not touched.


## Why it runs before gate-and-fix

Dead code inflates the surface the gates measure, and zombie tests provide exactly the part that makes dead code "look
exercised". Deleting first means the gates measure the real surface. In the reverse order, gate-and-fix proves a tree
that is changed right afterwards, and the most expensive stage runs twice.

The same holds between the two tracks: the dead-code track lands first, and the duplicate track discovers on the cleaned
tree; otherwise it would consolidate dead copies as if they were live duplicates.

Landing belongs to this skill itself, whoever invokes it. Letting the caller commit for us sounds more flexible and is
actually a trap: when the deletion is done the worktree is dirty, and the next stage (the duplicate track's preflight,
`gate-and-fix`) requires a clean tree and stops on the spot. Observed: in `pr`'s orchestration there was no commit stage
at all between the dead-code stage and the gates; the only commit was before the rebase. Whoever produces the change
lands it, so the handoff surface leaks nothing.

## How far each unit verifies itself

Wrong deletions first show up in **the module's own tests**, so running unit / integration tests is SOP, not optional.
But this is deliberately lighter than `gate-and-fix`:

- Run: the module's own tests, narrowed (Rust `just test-one <filter>`, Python `python3 -m unittest scripts.<name>_test` /
  `python3 -m pytest skills/<skill> -q`, Bun `bun test <file>`).
- Run: `rustfmt --check --edition 2021 <files you changed>`. **Never whole-tree `cargo fmt --check`**: it pulls in
  in-flight files of other agents in the same round; an agent reporting errors for another agent's half-finished work has
  already been observed.
- Do not run: coverage, E2E, whole-tree clippy / build, or any convergence rerun loop.

Component agents of the duplicate track do the same, with "the module" replaced by **every** module in their owner set:
a consolidation changes all users, and any user's tests may show the breakage first.

The division of labor is clear: this only proves "what I touched did not break the module I own"; overall gates belong
to `gate-and-fix`, which runs after this skill and measures the real surface after the cleanup.

## Stop conditions

On any of the following, stop and report; never mask it with a retry in the next round:

- The scope check exits nonzero (an artifact is out of scope).
- A module agent reports a blocker, or needs a shared-config change. ("The deletion crosses out of my module" is no
  longer a stop condition; it goes through HANDOFF.)
- Two agents' allowlists overlap: that is an orchestration error, not something to coordinate. In the duplicate track,
  groups whose owner sets intersect must be merged into one component before dispatch.
- A round in which no module produced the expected change and no agent reported CLEAN.
