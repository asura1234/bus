# Full-review procedure

Execute this procedure only from section 5 of the [entrypoint](../SKILL.md),
with its captured context and section 1 verification boundary.
GOTO REPORT continues at section 7 of that entrypoint.

```text
IF MODE == full:
  IF TARGET_KIND == scope:
    Review every declared whole committed file across all 9 dimensions, including code absent from
    the branch diff; record any incomplete coverage explicitly. Declared new probe paths have no
    committed code yet. Only dimensions 2 / 3 / 7 write and run red probes, restricted to SCOPE_TEST_FILES.
  ELSE:
    Inspect the non-plan diff, partition goal-serving and out-of-goal slices, and read every eligible
    goal-serving touched file in full. For out-of-goal slices, inspect only enough evidence to establish
    purpose, cumulative size, and affected paths for dimension 6.
  Use only direct callers, tests, and applicable architecture sources of truth as the upstream/downstream evidence needed for judgment;
  untouched modules with no direct contract relationship do not enter finding scope.

  IF runtime supports subagent dispatch:
    The user invoking this skill explicitly authorizes narrow fan-out for this round (write permission as in §1, limited to test files).
    Split the 9 dimensions into the fewest necessary groups by diff size:
      structure = 1, 6, 8
      correctness = 2, 3, 7
      hygiene = 4, 5, 9
    At most one subagent per group; small diffs may merge groups or be done serially by the main agent.
    Every **dimension-group** subagent must (except the fresh-eyes subagent, see below):
      - Read(docs/guides/code-review-guide.md) and docs/guides/architecture-principles.md
      - review only its assigned dimensions
      - Read(DIFF_SNAPSHOT); read the full current state of in-goal touched files; for dimension 6's out-of-goal slices only judge purpose / size
      - obey §1 VERIFICATION BOUNDARY: only the correctness group writes a suspicion as a test and runs it narrowly
        before deciding whether to report, writing only test files and running no quality gates; structure / hygiene groups are read-only
      - the main agent first assigns non-overlapping test files; name them semantically, never putting lane, dimension group, or round into test names
        In scope mode every assignment is a subset of SCOPE_TEST_FILES; all groups receive the same
        SCOPE_FILE / SCOPE_SNAPSHOT, whole-file target, and permissions, including fresh-eyes.
      - the main agent passes locked_goal / non_goals / archived_decisions and a goal-related narrow task; the structure group additionally
        does only dimension 6 classification on out-of-goal diff; other groups stop investigating on any unrelated item
      - return raw candidates: path:line / dimension / observation / evidence / impact / optional remediation

    **You must also dispatch one fresh-eyes subagent** (rationale in guide.md "Fan-out increases coverage"):
      - its ground truth is **verbatim identical** to the other groups: the same DIFF_SNAPSHOT, the same locked_goal /
        non_goals. The only allowed difference is "no checklist"
      - it **must** read guide.md "Write behavioral suspicion as a test first, then as a finding" — the proof boundary is written only there;
        not reading it means being unconstrained, and what gets loosened is exactly the rule that most needs to bind fresh-eyes
      - it **must not** read the rest of guide.md, code-review-guide.md, architecture-principles.md,
        the dimension list, or any prior-round review
      - §1 and the guide's proof boundary apply to it **in full**, without loosening a single word
    Claude Code may additionally use the built-in /code-review as a candidate source.
    All sources produce only candidates, never each their own review.md.
    The main agent re-verifies each against source and goal relevance, deduplicates by location + semantic root cause, discards out-of-scope items outside dimension 6 and nitpicks,
    then closes all subagents of this round.
  ELSE:
    The main agent covers the 9 dimensions serially.

  GOTO REPORT
```
