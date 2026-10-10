# Workflow: auto-merge-pr

This workflow is a skill for the orchestrator: it shows how the developer wants
one PR steered to "PR merged". Follow its steps and decisions, and use your own
judgment for everything it leaves open: agent names, staffing, and how to
recover.

## Graph

```mermaid
flowchart TD
    begin(["Get PR merge-ready"]):::start --> askverify
    askverify["Orchestrator: ask developer to merge without asking, or hold for manual test and approval"] --> measure
    measure["Orchestrator: measure added + removed lines against the PR base branch, excluding plans/"] --> big{"Total changed lines >= 3000?"}
    big -- yes --> split["Author: run split-pr --publish"]
    big -- no --> lockgoal["Orchestrator: get the PR goal and non-goals locked (ask the developer once if none is locked)"]
    lockgoal --> review
    split --> approveplan["Orchestrator: approve the split plan (split-pr CONFIRM)"]
    approveplan --> splitok{"split-pr built every part and published every publishable one?"}
    splitok -- no --> splitfailed(["Split failed (orchestrator recovers)"]):::failure
    splitok -- yes --> splitdone(["PR split (orchestrator recovers)"]):::failure

    subgraph round [Review round]
        review["Reviewers: run review-pr in parallel, each in its own lane; skip reviewers that gave Ready"]
        review --> allready{Every reviewer has given Ready?}
        allready -- no --> anyabandon{Any Abandon?}
        anyabandon -- "no (Needs Refinement)" --> address[Author: run address-review-comments, push]
        address --> outscope{"Orchestrator: any finding outside the PR goal?"}
        outscope -- yes --> recordscope[Orchestrator: record it in deferred.md]
        outscope -- no --> flag
        recordscope --> flag{Any FLAG?}
        flag -- yes --> triage[Orchestrator: triage every flagged issue]
        triage --> needsdev{"Orchestrator: any flagged issue blocking (unclear product requirements, an architectural decision, expected behavior)?"}
        needsdev -- no --> poll[Orchestrator: collect recommendations from author and reviewers, forward them to author]
        poll --> bestof[Author: run best-of-n on every flagged issue]
        bestof --> handback{best-of-n handed a question back?}
        handback -- no --> applyfirst[Author: apply the top-ranked option, push]
        flag -- no --> review
        applyfirst --> review
        applydec[Author: apply developer's feedback, push] --> review
    end

    needsdev -- yes --> informdev
    handback -- yes --> informdev

    anyabandon -- yes --> abandonfix{"Orchestrator: would splitting with split-pr fix every Abandon?"}
    abandonfix -- yes --> split
    abandonfix -- no --> informdev

    allready -- yes --> verify{"Developer asked to hold before merge? (start answer = merge permission)"}
    verify -- yes --> holdmerge["Author: run merge-pr --hold-before-merge"]
    holdmerge --> holding{merge-pr ready and holding?}
    holding -- no --> informdev
    holding -- yes --> manualtest[Developer: manual test and merge approval]:::developer
    manualtest --> approved{Approved?}
    approved -- yes --> merge
    approved -- no --> applydec
    verify -- no --> merge[Author: run merge-pr]
    merge --> mergeok{merge-pr merged the PR?}
    mergeok -- no --> informdev
    mergeok -- yes --> mainco{PR worktree is the main checkout?}
    mainco -- no --> closewt[Orchestrator: run worktree-close on the PR worktree]
    mainco -- yes --> anydeferred
    closewt --> anydeferred{Any entries in deferred.md?}
    anydeferred -- yes --> informmerged[Orchestrator: inform developer in MASTER, link deferred.md]
    anydeferred -- no --> merged(["PR merged"]):::success
    informmerged --> merged

    informdev["Orchestrator: inform developer in MASTER (reason, link deferred.md)"] --> blocked(["PR merge blocked (developer recovers)"]):::failure

    classDef start fill:#dbeafe,stroke:#1d4ed8
    classDef success fill:#dcfce7,stroke:#15803d
    classDef failure fill:#fee2e2,stroke:#b91c1c
    classDef developer fill:#fde2e4,stroke:#c9184a
```

`deferred.md` is `<main checkout>/temp/<branch-slug>/deferred.md`, where every
character of the branch name outside `A-Za-z0-9_-` becomes `-`. It holds only
findings outside the PR goal, each with what it is and why it is out of scope.
worktree-close never removes the main checkout's `temp/`, so the link survives.

best-of-n is there to keep the developer out of failure resolution: whatever
its verdict (universal, clear or toss-up), the author applies the top-ranked
option. best-of-n keeps the ranking in its own ledger
(`temp/best-of-n/<branch-slug>/ledger.json`, rendered as `decisions.md`).
Only blocking flagged issues and questions best-of-n hands back reach the
developer.

## Failure recovery

- **PR merge blocked (developer recovers):** the developer starts from the
  reason you posted in MASTER, `deferred.md`, and the PR state (branch, open
  PR, latest review round, the merge-pr report if it ran).

## Orchestrator failure recovery

You handle these yourself:

- **PR split:** this run merges nothing; the parts replace the original PR.
  Run this workflow once per part and carry over the start answer instead of
  asking again. Start a part once all its parents have merged; parts with no
  open parent can run in parallel, each with its own author and reviewers.
  A part with two or more parents stays a local branch under split-pr's
  default `wait` policy until restack leaves it one base; that is expected.
  A train merges strictly bottom up, never into a parent branch; after a
  parent merges, have an agent run `split-pr restack <plan> --publish` with
  the plan path (`temp/split-pr/<source-slug>/plan.json` in the worktree where
  split-pr ran, so keep that worktree until every part is done). split-pr
  keeps the source branch until the developer authorizes cleanup; once every
  part has its PR, ask the developer in MASTER to close the original PR.
- **Split failed:** rerunning split-pr STOPs while its plan exists
  (PREFLIGHT). Have an agent fix the plan or the branch and rerun the helper
  step that failed; for a failed publish wave, restack mode with the plan path
  publishes the parts that are now publishable. A part that only a redesign
  could make independent is the developer's call.
- **Usage limit mid-run:** judge by the provider's reset time. Swap another
  model into that lane, keeping the reviewers on different models, or wait for
  the reset.

## Participants

| Role | Model | Does |
|---|---|---|
| Orchestrator | any | Steers the run to "PR merged" and recovers from the cases above. Names and staffs the agents and changes the team as the run needs. Asks the merge-permission question once; that answer is the permission to merge. Gets the PR goal and non-goals locked, approves the split plan, decides whether splitting fixes an Abandon, records out-of-scope findings in `deferred.md`, triages flagged issues, and on the developer's behalf has the author apply best-of-n's top-ranked option. Informs the developer in MASTER at "PR merge blocked", and at "PR merged" when `deferred.md` has entries. Writing `deferred.md` and running worktree-close are its only hands-on actions; it delegates everything else. |
| Developer | - | Answers the merge-permission question; supplies the PR goal and non-goals when none is locked; optionally runs the manual test and merge approval while merge-pr holds; recovers from "PR merge blocked". |
| Author | any | split-pr, address-review-comments (which commits the reviewers' tests), best-of-n, fixes, merge-pr (which handles CI and new GitHub review comments, pushing its own fixes). Before split-pr or merge-pr, commits any reviewer tests still left in the worktree. |
| Reviewers | different models: at least Claude and Codex, Cursor optional | review-pr, each with `--reviewer` set to its own agent name. |

All agents for one PR share its worktree (any worktree, not necessarily the
main checkout); the author is the only one who commits.
