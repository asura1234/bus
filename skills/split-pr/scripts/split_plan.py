#!/usr/bin/env python3
"""Mechanics for the split-pr plan artifact.

The plan (references/split-plan-format.md) records which source commits go to
which part, how parts depend on each other, and, once branches exist, the commit
each part's own commits sit on. Every command fails closed: a malformed plan,
an unknown commit, a cycle, or a branch that does not sit where the plan says
is an error, never a warning.

Commands:
  check     validate the plan against git and print its shape
  render    print the confirmation view: order, table, mermaid graph
  probe     replay each part onto the base with git merge-tree (no checkout)
  record    store a built branch's onto/tip (and PR number or landing)
  restack   print the ordered rebase steps after a parent changed or landed
  stack     print the stack bullet for one part's PR description
  coverage  prove the union of all part branches equals the source tree
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from pathlib import Path

from split_plan_git import (
    change_balance,
    check_against_git,
    commit_files,
    git,
    is_ancestor,
    merge_tree,
    part_size,
    range_commits,
    replay,
    rev,
    synthetic_commit,
)
from split_plan_model import (
    Part,
    Plan,
    PlanError,
    _require,
    base_label,
    closure,
    integration_refs,
    load,
    open_parents,
    save,
    shape,
    shared_commits,
    topo_order,
)
from split_plan_stack import cmd_restack, cmd_stack


# --- commands ----------------------------------------------------------------


def cmd_check(repo: Path, plan: Plan) -> dict:
    check_against_git(repo, plan)
    return {
        "status": "ok",
        "shape": shape(plan),
        "order": [p.id for p in topo_order(plan)],
        "shared_commits": sorted(shared_commits(plan)),
    }


def cmd_render(repo: Path, plan: Plan) -> str:
    check_against_git(repo, plan)
    order = topo_order(plan)
    lines = [
        f"Shape: {shape(plan)} ({len(order)} parts, multi-parent policy `{plan.multi_parent}`)",
        f"Source: `{plan.source_branch}` @ {plan.source_sha[:12]}; base: `{plan.base_ref}` @ {plan.base_sha[:12]}",
        "",
        "| # | Part | Branch | Base | Depends on | Commits | Files | Lines |",
        "|---|---|---|---|---|---|---|---|",
    ]
    shared = shared_commits(plan)
    position = {sha: i for i, sha in enumerate(range_commits(repo, plan.base_sha, plan.source_sha, merges=False))}
    file_lists: list[str] = []
    estimated = False
    for index, part in enumerate(order, 1):
        files, added, deleted, churn = part_size(repo, plan, part, position)
        estimated = estimated or churn
        commits = ", ".join(sha[:9] + ("*" if sha in shared else "") for sha in part.commits)
        deps = ", ".join(part.depends_on) or "—"
        approx = "~" if churn else ""
        lines.append(
            f"| {index} | {part.id}: {part.title} | `{part.branch}` | `{base_label(plan, part)}` "
            f"| {deps} | {commits} | {approx}{len(files)} | {approx}+{added} / -{deleted} |"
        )
        file_lists.append(f"- {part.id}: {', '.join(files)}")
    total = git(repo, "diff", "--shortstat", f"{plan.base_sha}...{plan.source_sha}").strip()
    lines += ["", f"Source total: {total or 'no changes'}"]
    if shared:
        lines.append("")
        lines.append("`*` commit shared by several parts: split by hunk, manual reconstruction.")
    if estimated:
        lines.append("")
        lines.append(
            "`~` the part's ancestry has a shared commit or does not replay cleanly, so its sizes "
            "sum each whole commit's churn and can overestimate the PR diff."
        )
    lines += ["", "Files per part:", *file_lists]
    if plan.left_on_source:
        lines.append("")
        lines.append("Left on source:")
        lines += [f"- {sha[:9]}: {reason}" for sha, reason in zip(plan.left_on_source, plan.left_reasons)]
    lines += ["", "```mermaid", "flowchart BT", f"    base[({plan.base_ref})]"]
    for part in order:
        lines.append(f'    {part.id}["{part.id}: {part.branch}"]')
    for part in order:
        if not part.depends_on:
            lines.append(f"    {part.id} --> base")
        for dep in part.depends_on:
            lines.append(f"    {part.id} --> {dep}")
    lines.append("```")
    return "\n".join(lines)


def cmd_probe(repo: Path, plan: Plan) -> dict:
    check_against_git(repo, plan)
    in_range = range_commits(repo, plan.base_sha, plan.source_sha, merges=False)
    position = {sha: i for i, sha in enumerate(in_range)}
    shared = shared_commits(plan)
    owner = {sha: p.id for p in plan.parts for sha in p.commits if sha not in shared}
    results = []
    failed = False
    for part in topo_order(plan):
        entry: dict[str, object] = {"part": part.id, "declared": list(part.depends_on)}
        ancestry = closure(plan, part)
        if any(sha in shared for pid in ancestry for sha in plan.part(pid).commits):
            entry["status"] = "manual"
            results.append(entry)
            continue
        alone = replay(repo, plan.base_sha, list(part.commits)) is not None
        with_deps_commits = sorted(
            {sha for pid in ancestry for sha in plan.part(pid).commits}, key=position.__getitem__
        )
        with_deps = replay(repo, plan.base_sha, with_deps_commits) is not None
        files = {f for sha in part.commits for f in commit_files(repo, sha)}
        first = min(position[sha] for sha in part.commits)
        overlaps = sorted(
            {
                owner[sha]
                for sha in in_range[:first]
                if sha in owner and owner[sha] not in ancestry and files & set(commit_files(repo, sha))
            }
        )
        entry.update(
            {
                "alone": "clean" if alone else "conflict",
                "with_deps": "clean" if with_deps else "conflict",
                "overlaps": overlaps,
            }
        )
        if not with_deps:
            entry["status"] = "missing-dependency"
            failed = True
        elif part.depends_on and alone:
            entry["status"] = "build-check-dependency"
        elif part.depends_on:
            entry["status"] = "textual-dependency"
        else:
            entry["status"] = "independent"
        results.append(entry)
    return {"status": "fail" if failed else "ok", "parts": results}


def expected_onto(repo: Path, plan: Plan, part: Part, explicit: str | None) -> str:
    parents = open_parents(plan, part)
    refs = integration_refs(plan, part)
    if len(refs) >= 2:
        _require(explicit is not None, f"part `{part.id}` sits on {base_label(plan, part)}; pass --onto <merge commit>")
        onto = rev(repo, explicit or "")
        merge_parents = set(git(repo, "rev-list", "--parents", "-n", "1", onto).split()[1:])
        tips = {rev(repo, ref if ref == plan.base_ref else f"refs/heads/{ref}") for ref in refs}
        _require(merge_parents == tips, f"--onto must be a merge of exactly the current tips of {refs}")
        return onto
    if len(parents) == 1:
        candidates = [rev(repo, f"refs/heads/{parents[0].branch}")]
    else:
        candidates = [rev(repo, plan.base_ref), plan.base_sha]
    if explicit is not None:
        onto = rev(repo, explicit)
        _require(
            onto in candidates, f"--onto {explicit} is not the expected base of `{part.id}` ({base_label(plan, part)})"
        )
        return onto
    tip = rev(repo, f"refs/heads/{part.branch}")
    for candidate in candidates:
        if is_ancestor(repo, candidate, tip):
            return candidate
    raise PlanError(f"branch `{part.branch}` does not sit on {base_label(plan, part)}")


def cmd_record(repo: Path, path: Path, data: dict, plan: Plan, args: argparse.Namespace) -> dict:
    part = plan.part(args.part)
    raw = next(p for p in data["parts"] if p["id"] == part.id)
    if args.landed:
        _require(raw["pr"] is not None or args.pr is not None, f"part `{part.id}` has no PR to have landed")
        raw["landed"] = True
    else:
        tip = rev(repo, f"refs/heads/{part.branch}")
        _require(tip != "", f"branch `{part.branch}` does not exist")
        onto = expected_onto(repo, plan, part, args.onto)
        _require(is_ancestor(repo, onto, tip), f"`{part.branch}` does not contain its base {onto[:12]}")
        own = range_commits(repo, onto, tip, merges=False)
        _require(own, f"`{part.branch}` has no commits of its own on {onto[:12]}")
        _require(not range_commits(repo, onto, tip, merges=True), f"`{part.branch}` has merge commits above its base")
        raw["onto"], raw["tip"] = onto, tip
    if args.pr is not None:
        raw["pr"] = args.pr
    save(path, data)
    return {
        "status": "recorded",
        "part": part.id,
        "onto": raw["onto"],
        "tip": raw["tip"],
        "pr": raw["pr"],
        "landed": raw["landed"],
    }


def cmd_coverage(repo: Path, plan: Plan) -> dict:
    state = plan.base_sha
    for part in topo_order(plan):
        _require(part.tip is not None, f"part `{part.id}` was never recorded")
        tip = rev(repo, f"refs/heads/{part.branch}")
        _require(
            is_ancestor(repo, plan.base_sha, tip),
            f"`{part.branch}` does not contain base.sha; run coverage before restacking",
        )
        tree = merge_tree(repo, state, tip, None)
        _require(tree is not None, f"merging `{part.branch}` into the union conflicts")
        assert tree is not None
        state = synthetic_commit(repo, tree, [state, tip], f"union {part.id}")
    expected = plan.source_sha
    if plan.left_on_source:
        # Compare against the source without the left commits, so a dropped hunk
        # or a stray file cannot hide behind a path those commits also touch.
        in_range = range_commits(repo, plan.base_sha, plan.source_sha, merges=False)
        left = set(plan.left_on_source)
        replayed = replay(repo, plan.base_sha, [sha for sha in in_range if sha not in left])
        _require(
            replayed is not None, "the assigned commits do not apply without left_on_source; a part needs a left commit"
        )
        assert replayed is not None
        expected = replayed
    diff = git(repo, "diff", "--name-only", state, expected)
    if diff:
        return {"status": "fail", "differs": diff.splitlines()}
    # The union can equal the expected tree while a hunk or tree entry sits in the
    # wrong part or in two parts at once (identical changes merge cleanly).
    wrong: set[str] = set()
    shared = shared_commits(plan)
    total: Counter[tuple[bytes, bytes, bytes]] = Counter()
    for part in plan.parts:
        assert part.onto is not None
        tip = rev(repo, f"refs/heads/{part.branch}")
        total.update(change_balance(repo, part.onto, tip))
        if not any(sha in shared for sha in part.commits):
            replayed_part = replay(repo, part.onto, list(part.commits))
            _require(replayed_part is not None, f"part `{part.id}` commits do not replay onto its recorded base")
            assert replayed_part is not None
            own = git(repo, "diff", "--name-only", replayed_part, tip)
            wrong.update(own.splitlines() if own else [])
    total.subtract(change_balance(repo, plan.base_sha, expected))
    wrong.update(path.decode(errors="replace") for (path, _kind, _value), count in total.items() if count != 0)
    if wrong:
        return {"status": "fail", "differs": sorted(wrong)}
    left_diff = git(repo, "diff", "--name-only", state, plan.source_sha)
    paths = left_diff.splitlines() if left_diff else []
    return {"status": "ok" if not paths else "review-left-on-source", "differs": paths}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("check", "render", "probe", "restack", "coverage"):
        sub.add_parser(name).add_argument("plan", type=Path)
    record = sub.add_parser("record")
    record.add_argument("plan", type=Path)
    record.add_argument("--part", required=True)
    record.add_argument("--onto")
    record.add_argument("--pr", type=int)
    record.add_argument("--landed", action="store_true")
    stack = sub.add_parser("stack")
    stack.add_argument("plan", type=Path)
    stack.add_argument("--part", required=True)
    args = parser.parse_args(argv)
    repo = args.repo
    try:
        plan, data = load(args.plan)
        if args.command == "render":
            print(cmd_render(repo, plan))
            return 0
        if args.command == "stack":
            print(cmd_stack(plan, args.part))
            return 0
        if args.command == "check":
            result = cmd_check(repo, plan)
        elif args.command == "probe":
            result = cmd_probe(repo, plan)
        elif args.command == "record":
            result = cmd_record(repo, args.plan, data, plan, args)
        elif args.command == "restack":
            result = cmd_restack(repo, plan)
        else:
            result = cmd_coverage(repo, plan)
    except PlanError as error:
        print(f"split_plan: {error}", file=sys.stderr)
        return 2
    print(json.dumps(result, indent=2))
    return 1 if result.get("status") == "fail" else 0


if __name__ == "__main__":
    sys.exit(main())
