---
name: pr
description: Feature-branch PR workflow. Land relevant uncommitted work, rebase onto origin/master, immediately create or refresh an English Draft PR, converge repository gates, synchronize documentation, and publish a deterministically validated final body. Dead-code cleanup is opt-in through --delete-dead-code. Use when asked to open, create, submit, update, or generate a PR description.
---

# PR

```text
INPUT $ARGUMENTS = [--plan <plan-file>]... [--delete-dead-code]
plans = every plan path supplied by the developer for this invocation, in argument order; never infer plans from diff, commits, or historical state.

RULES
- Code narrative uses only the current repository, feature branch, and actual changes as facts. Plans lock goal and non-goals only.
- Prior create-plan, execute-plan, review-plan, or review-pr invocation is not required.
- Do not inspect or infer another skill's private control state, callbacks, or temporary artifacts. The sole cross-skill handoff is `.locked-goal` and `.locked-non-goals`, written and returned by pr_goal_context.py.
- PR title and narrative content are English. Canonical fixed headings and machine tokens remain byte-compatible with references/pr-template.md. Both the initial Draft and final body must pass pr_format_check.py with the matching phase.
- Every commit and push is delegated to commit-and-push. Every rebase is delegated to rebase-origin-main. Do not duplicate their Git protocols.

========== PREFLIGHT ==========

repo = git rev-parse --show-toplevel
branch = git branch --show-current

IF repo missing:
  ERROR "The current directory is not a Git repository."
IF branch empty OR branch IN {main, master}:
  ERROR "A PR cannot be created from detached HEAD or main/master."

Run:
  git status --short
  git fetch origin
Assert origin/master resolves.

working_changed = tracked + untracked files in the current worktree
committed_changed = git diff --name-only origin/master...HEAD
ahead = git rev-list --count origin/master..HEAD

IF working_changed empty AND committed_changed empty AND ahead == 0:
  ERROR "The current branch has no work to commit or publish relative to origin/master."

Inspect the complete current diff, untracked files, and commits. Confirm they express one coherent PR purpose. Never silently discard, stash, rewrite, or absorb unrelated developer changes.

========== LAND UNCOMMITTED WORK (ONLY IF DIRTY) ==========

IF the worktree has uncommitted changes belonging to this PR:
  invoke commit-and-push;
  it preserves unrelated dirty files, splits commits by single purpose, and pushes the explicit same-name refspec.

  Assert:
    - every intended PR file is committed;
    - unrelated pre-existing dirty files remain untouched and uncommitted;
    - the remote feature branch contains local HEAD.
ELSE:
  skip directly to rebase. Do not invoke commit-and-push merely to report no work.

This stage exists only to make rebase possible. Unrelated dirty files remain uncommitted. If they block rebase, report the blocker instead of absorbing them.

gate-and-fix later requires a clean worktree to bind its artifact to one committed tree. If unrelated dirty files remain, STOP before readiness convergence and report them as excluded.

========== REBASE ==========

Invoke rebase-origin-main with the Bus adapter base `origin/master`.
It owns conflict triage, dependency/submodule resynchronization when applicable, and force-with-lease push. This skill does not run rebase itself. If it reports dropped shared infrastructure, carry that text verbatim into PR verification/risk notes.

Rebase precedes readiness gates. Gates must prove the merged result; pre-rebase gates prove the wrong tree and rerunning them later duplicates expensive work.

baseline = git rev-parse origin/master
draft_head = git rev-parse HEAD
Assert the remote branch tip equals draft_head.
draft_diff = git diff <baseline>...<draft_head>
draft_commits = git log <baseline>..<draft_head> --oneline

========== LOCK PR INTENT ==========

Read(docs/guides/orchestrated-room-brief.md) completely, then run:
  python3 cli_extensions/room_assignment_context.py [--frame "<frame>"] \
    --output "<ignored-temp-assignment-context>"
IF exit != 0:
  STOP and return stdout verbatim as blocker evidence.
ORIGIN = ASSIGNMENT_ORIGIN

IF ORIGIN == verified:
  Run:
    python3 skills/pr/scripts/pr_goal_context.py \
      --branch "<branch>" --output "<ignored-temp-goal-context>" \
      --assignment-context "<ignored-temp-assignment-context>" \
      [--plan "<path>" ... in supplied order]
  Supplied plans must match the verified Room Brief exactly; the script fails closed otherwise.
ELSE IF ORIGIN == NotInBusRoom AND plans nonempty:
  Run:
    python3 skills/pr/scripts/pr_goal_context.py \
      --branch "<branch>" --output "<ignored-temp-goal-context>" \
      [--plan "<path>" ... in supplied order]
ELSE:
  derive one goal from draft_diff / draft_commits;
  include only evidence-backed intentional exclusions as non-goals, otherwise use `无`;
  write ignored temporary goal and non-goal files, then Run:
    python3 skills/pr/scripts/pr_goal_context.py \
      --branch "<branch>" --output "<ignored-temp-goal-context>" \
      --goal-file "<goal-file>" --non-goal-file "<non-goal-file>"

IF the command fails:
  STOP. Do not hand-parse plans or create the lock manually.

Capture GOAL_CONTEXT_FILE, LOCKED_GOAL_FILE, and LOCKED_NON_GOALS_FILE.
Read(GOAL_CONTEXT_FILE) completely.
The generated goal/non-goal sections and both locks are immutable for the rest of this run.

========== PUBLISH DRAFT OR CAPTURE EXISTING READY PR ==========

Read(references/pr-template.md) completely.
Derive the initial English title and non-intent narrative only from immutable draft_diff / draft_commits. Take goal and non-goals only from GOAL_CONTEXT_FILE. Fill every template section. Documentation synchronization uses pending unchecked items; testing retains the pending readiness gate; notes say this is the post-rebase snapshot and selected cleanup/gates/docs remain pending. Never mark unperformed work passed.

Write draft_body_file under ignored temp, then Run:

  python3 skills/pr/scripts/pr_format_check.py --phase draft \
    --template skills/pr/references/pr-template.md --title "<draft-title>" \
    --body-file "<draft-body-file>" --goal-context-file "<GOAL_CONTEXT_FILE>" \
    --locked-goal-file "<LOCKED_GOAL_FILE>"

IF validation fails:
  repair and rerun; never publish an invalid Draft.

existing = gh pr list --head <branch> --state open --json number,title,url,isDraft
STOP if more than one PR exists.
created_as_draft = false

IF existing Draft PR:
  gh pr edit <number> --title "<draft-title>" --body-file <draft-body-file>
ELSE IF existing ready PR:
  capture it but preserve title, body, and ready status until finalization
ELSE:
  gh pr create --draft --head <branch> --base master --title "<draft-title>" --body-file <draft-body-file>
  created_as_draft = true

Capture PR number and URL. A PR created here remains Draft; never run gh pr ready. Draft create/edit occurs before dead-code cleanup, gate-and-fix, or update-docs. An existing ready PR is captured now and edited only during finalization. If a later stage blocks, preserve current Draft/ready state and report its URL plus the blocker. This PR request authorizes only create/edit and final edit, never approve, merge, close, or delete.

========== DEAD CODE (OPT-IN) ==========

IF --delete-dead-code absent:
  skip and record that fact in final notes.
ELSE:
  invoke delete-dead-code --base <baseline>;
  it owns scope derivation, fan-out, scope verification, and landing through commit-and-push;
  carry its LIKELY / KEPT items into the final PR body;
  if blocked or out-of-scope changes appear, STOP and report instead of gating an unverified deletion tree.

========== READINESS CONVERGENCE ==========

Invoke gate-and-fix --base <baseline>.
It owns the complete concurrent gate/remediation/commit-and-push loop and the E2E prohibition. Capture its final PASS artifact and Head as gated_head. Movement of origin/master after baseline does not trigger another rebase.

copy_diff = git diff <baseline>...<gated_head>
copy_commits = git log <baseline>..<gated_head> --oneline

========== FINAL FAN-OUT ==========

In the same turn, dispatch two subagents with disjoint ownership:

1. Documentation owner: invoke update-docs --base <baseline>. It is the only tracked-worktree writer, preserves the finalized audit path, and performs no Git or GitHub mutation.
2. PR finalizer: remain repository-read-only and derive the final English title and non-intent narrative only from immutable copy_diff / copy_commits. Goal and non-goals come only from GOAL_CONTEXT_FILE.

   Exclude the later update-docs-only delta and commit from title, size, summary, and change list. It may write the body only under ignored temp, stays available for the finalized audit, and owns final pr_format_check plus gh pr edit for the captured PR.

The finalizer must not read live worktree files, live HEAD, or mutable origin/master as narrative authority. The documentation owner stops on overlapping active ownership. Neither subagent commits or pushes.

Wait for the documentation owner. Preserve its audit path and run git diff --check. If docs fail, return exact failures to that owner; it fixes only owned files, invalidates the old audit, and reruns update-docs to create a replacement audit. Repeat until clean. Assert every worktree change is an audit target or required agent-instruction symlink. Any other delta blocks. Capture normalized path/status as docs_delta.

Send the finalized audit path and diff-check result to the waiting PR finalizer. It fills every template section:

- Insert the goal and non-goal sections from GOAL_CONTEXT_FILE verbatim, without labels, summaries, translations, or separators.
- Populate the documentation section by running the canonical docs audit renderer against the latest audit and inserting stdout verbatim.
- Populate testing only with the final PASS gate-and-fix artifact and Head, the finalized documentation audit plus git diff --check, and other verification actually run in this invocation. Every item is checked. Put unperformed E2E, review, or manual verification in notes as pending.

Write final_body_file under temp, then Run:

  python3 skills/pr/scripts/pr_format_check.py --phase final \
    --template skills/pr/references/pr-template.md --title "<final-title>" \
    --body-file "<final-body-file>" --goal-context-file "<GOAL_CONTEXT_FILE>" \
    --locked-goal-file "<LOCKED_GOAL_FILE>"

IF validation fails:
  repair and rerun; never replace the Draft with an invalid final body.

Assert remote feature branch tip is gated_head, then edit title/body without changing Draft status. Report PR_FINALIZED and remain available.

========== LAND FINAL DOCUMENTATION ==========

IF update-docs changed documentation:
  invoke commit-and-push once.

final_head = git rev-parse HEAD
final_status = git status --short
branch = git branch --show-current

Assert the worktree is clean, remote feature branch contains final_head, and every gated_head..final_head path/status equals docs_delta. Do not regenerate the narrative from this docs-only commit.

Send final_head to the waiting PR finalizer. It reads the published PR state and writes published body byte-exact under ignored temp, then reruns final pr_format_check.

IF published title/body differs from intended content or fails validation:
  repair with gh pr edit and re-verify.

Require published head == final_head, state == OPEN, base == master, and a PR created here remains Draft. Otherwise STOP with observed fields.

========== RETURN ==========

Report:
  - PR title and URL
  - publication status and Draft behavior
  - final HEAD
  - commits created or updated
  - final PASS gate artifact with Base/Head and finalized docs audit plus git diff --check
  - GOAL_CONTEXT_FILE, LOCKED_GOAL_FILE, and LOCKED_NON_GOALS_FILE for review-pr
  - pr_format_check result on the published PR
  - pending manual verification
  - unrelated dirty files explicitly excluded
```
