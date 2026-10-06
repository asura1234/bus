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
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

SCHEMA = "split-pr-plan/1"
MULTI_PARENT_POLICIES = ("wait", "merge")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
TOP_KEYS = {"schema", "source", "base", "multi_parent", "parts", "left_on_source"}
PART_KEYS = {"id", "branch", "title", "depends_on", "commits", "onto", "tip", "pr", "landed"}


class PlanError(Exception):
    pass


@dataclass(frozen=True)
class Part:
    id: str
    branch: str
    title: str
    depends_on: tuple[str, ...]
    commits: tuple[str, ...]
    onto: str | None
    tip: str | None
    pr: int | None
    landed: bool


@dataclass(frozen=True)
class Plan:
    source_branch: str
    source_sha: str
    base_ref: str
    base_sha: str
    multi_parent: str
    parts: tuple[Part, ...]
    left_on_source: tuple[str, ...]

    def part(self, part_id: str) -> Part:
        for part in self.parts:
            if part.id == part_id:
                return part
        raise PlanError(f"unknown part `{part_id}`")

    @property
    def github_base(self) -> str:
        return self.base_ref.removeprefix("origin/")


# --- git -------------------------------------------------------------------


def git(repo: Path, *args: str, check: bool = True, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        env={**os.environ, **env} if env else None,
    )
    if check and result.returncode != 0:
        raise PlanError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def rev(repo: Path, name: str) -> str:
    return git(repo, "rev-parse", "--verify", "--quiet", f"{name}^{{commit}}")


def is_ancestor(repo: Path, ancestor: str, descendant: str) -> bool:
    result = subprocess.run(
        ["git", "-C", str(repo), "merge-base", "--is-ancestor", ancestor, descendant],
        capture_output=True,
    )
    return result.returncode == 0


def range_commits(repo: Path, base: str, tip: str, *, merges: bool) -> list[str]:
    flag = "--merges" if merges else "--no-merges"
    out = git(repo, "rev-list", "--reverse", flag, f"{base}..{tip}")
    return out.split() if out else []


def commit_files(repo: Path, sha: str) -> list[str]:
    out = git(repo, "diff-tree", "--no-commit-id", "--name-only", "-r", sha)
    return out.splitlines()


def synthetic_commit(repo: Path, tree: str, parents: list[str], message: str) -> str:
    args = ["commit-tree", tree, "-m", message]
    for parent in parents:
        args += ["-p", parent]
    ident = {
        "GIT_AUTHOR_NAME": "split-pr",
        "GIT_AUTHOR_EMAIL": "split-pr@localhost",
        "GIT_COMMITTER_NAME": "split-pr",
        "GIT_COMMITTER_EMAIL": "split-pr@localhost",
    }
    return git(repo, *args, env=ident)


def merge_tree(repo: Path, ours: str, theirs: str, merge_base: str | None) -> str | None:
    """Return the merged tree, or None on conflict. Never touches a worktree."""
    args = ["merge-tree", "--write-tree", "--no-messages"]
    if merge_base is not None:
        args.append(f"--merge-base={merge_base}")
    result = subprocess.run(
        ["git", "-C", str(repo), *args, ours, theirs], capture_output=True, text=True
    )
    if result.returncode == 0:
        return result.stdout.split()[0]
    if result.returncode == 1:
        return None
    raise PlanError(f"git merge-tree failed: {result.stderr.strip()}")


def replay(repo: Path, onto: str, commits: list[str]) -> str | None:
    """Cherry-pick `commits` onto `onto` in object space; None on conflict."""
    state = onto
    for sha in commits:
        tree = merge_tree(repo, state, sha, f"{sha}^")
        if tree is None:
            return None
        state = synthetic_commit(repo, tree, [state], f"probe {sha}")
    return state


# --- loading and validation --------------------------------------------------


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise PlanError(message)


def parse_plan(data: object) -> Plan:
    _require(isinstance(data, dict), "plan must be a JSON object")
    assert isinstance(data, dict)
    _require(set(data) == TOP_KEYS, f"plan keys must be exactly {sorted(TOP_KEYS)}; found {sorted(data)}")
    _require(data["schema"] == SCHEMA, f"schema must be `{SCHEMA}`")
    source, base = data["source"], data["base"]
    for name, value in (("source", source), ("base", base)):
        _require(
            isinstance(value, dict) and set(value) == {"ref" if name == "base" else "branch", "sha"},
            f"`{name}` must have exactly {'ref' if name == 'base' else 'branch'} and sha",
        )
        _require(isinstance(value["sha"], str) and SHA_RE.match(value["sha"]) is not None, f"`{name}.sha` must be a full SHA")
    _require(isinstance(source["branch"], str) and source["branch"] != "", "`source.branch` is required")
    _require(
        isinstance(base["ref"], str) and base["ref"].startswith("origin/") and base["ref"] != "origin/",
        "`base.ref` must be an explicit origin/<branch> ref",
    )
    _require(data["multi_parent"] in MULTI_PARENT_POLICIES, f"`multi_parent` must be one of {MULTI_PARENT_POLICIES}")
    _require(isinstance(data["parts"], list) and len(data["parts"]) >= 2, "a split needs at least two parts")
    left = data["left_on_source"]
    _require(isinstance(left, list) and all(isinstance(s, str) and SHA_RE.match(s) for s in left), "`left_on_source` must be a list of full SHAs")
    _require(len(set(left)) == len(left), "`left_on_source` has duplicates")

    parts: list[Part] = []
    for index, raw in enumerate(data["parts"]):
        where = f"parts[{index}]"
        _require(isinstance(raw, dict) and set(raw) == PART_KEYS, f"{where} keys must be exactly {sorted(PART_KEYS)}")
        _require(isinstance(raw["id"], str) and ID_RE.match(raw["id"]) is not None, f"{where}.id must match {ID_RE.pattern}")
        _require(isinstance(raw["branch"], str) and raw["branch"] != "", f"{where}.branch is required")
        _require(isinstance(raw["title"], str) and raw["title"].strip() != "", f"{where}.title is required")
        deps, commits = raw["depends_on"], raw["commits"]
        _require(isinstance(deps, list) and all(isinstance(d, str) for d in deps), f"{where}.depends_on must be a list of ids")
        _require(len(set(deps)) == len(deps), f"{where}.depends_on has duplicates")
        _require(isinstance(commits, list) and commits != [], f"{where}.commits must be a non-empty list")
        _require(all(isinstance(s, str) and SHA_RE.match(s) for s in commits), f"{where}.commits must be full SHAs")
        _require(len(set(commits)) == len(commits), f"{where}.commits has duplicates")
        for key in ("onto", "tip"):
            value = raw[key]
            _require(value is None or (isinstance(value, str) and SHA_RE.match(value) is not None), f"{where}.{key} must be null or a full SHA")
        _require(raw["tip"] is None or raw["onto"] is not None, f"{where}.tip requires onto")
        pr = raw["pr"]
        _require(pr is None or (isinstance(pr, int) and not isinstance(pr, bool) and pr > 0), f"{where}.pr must be null or a positive integer")
        _require(isinstance(raw["landed"], bool), f"{where}.landed must be a boolean")
        _require(not raw["landed"] or pr is not None, f"{where} cannot be landed without a PR")
        parts.append(
            Part(raw["id"], raw["branch"], raw["title"], tuple(deps), tuple(commits), raw["onto"], raw["tip"], pr, raw["landed"])
        )

    ids = [p.id for p in parts]
    _require(len(set(ids)) == len(ids), "part ids must be unique")
    branches = [p.branch for p in parts]
    _require(len(set(branches)) == len(branches), "part branches must be unique")
    for part in parts:
        _require(part.branch not in {source["branch"], base["ref"], base["ref"].removeprefix("origin/")}, f"part `{part.id}` reuses the source or base branch name")
        for dep in part.depends_on:
            _require(dep in ids, f"part `{part.id}` depends on unknown part `{dep}`")
            _require(dep != part.id, f"part `{part.id}` depends on itself")
    plan = Plan(source["branch"], source["sha"], base["ref"], base["sha"], data["multi_parent"], tuple(parts), tuple(left))
    topo_order(plan)  # rejects cycles
    for part in parts:
        if part.landed:
            for dep in part.depends_on:
                _require(plan.part(dep).landed, f"part `{part.id}` is landed before its parent `{dep}`")
    return plan


def load(path: Path) -> tuple[Plan, dict]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise PlanError(f"cannot read plan {path}: {error}") from error
    return parse_plan(data), data


def save(path: Path, data: dict) -> None:
    parse_plan(data)
    fd, temp = tempfile.mkstemp(dir=path.parent, prefix=".plan-", suffix=".json")
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=2)
        handle.write("\n")
    os.replace(temp, path)


def topo_order(plan: Plan) -> list[Part]:
    """Kahn's algorithm; ties keep plan order so output is deterministic."""
    remaining = list(plan.parts)
    done: set[str] = set()
    order: list[Part] = []
    while remaining:
        ready = next((p for p in remaining if set(p.depends_on) <= done), None)
        if ready is None:
            raise PlanError("depends_on contains a cycle: " + ", ".join(p.id for p in remaining))
        order.append(ready)
        done.add(ready.id)
        remaining.remove(ready)
    return order


def children(plan: Plan, part_id: str) -> list[Part]:
    return [p for p in plan.parts if part_id in p.depends_on]


def shape(plan: Plan) -> str:
    if all(not p.depends_on for p in plan.parts):
        return "parallel"
    roots = [p for p in plan.parts if not p.depends_on]
    linear = all(len(p.depends_on) <= 1 and len(children(plan, p.id)) <= 1 for p in plan.parts)
    return "train" if linear and len(roots) == 1 else "mixed"


def open_parents(plan: Plan, part: Part) -> list[Part]:
    return [plan.part(d) for d in part.depends_on if not plan.part(d).landed]


def base_label(plan: Plan, part: Part) -> str:
    parents = open_parents(plan, part)
    if not parents:
        return plan.base_ref
    if len(parents) == 1:
        return parents[0].branch
    return "merge(" + ", ".join(p.branch for p in parents) + ")"


def check_against_git(repo: Path, plan: Plan) -> None:
    _require(is_ancestor(repo, plan.base_sha, plan.source_sha), "base.sha is not an ancestor of source.sha")
    merges = range_commits(repo, plan.base_sha, plan.source_sha, merges=True)
    _require(not merges, f"source range contains merge commits {merges}; linearize the source first")
    in_range = range_commits(repo, plan.base_sha, plan.source_sha, merges=False)
    position = {sha: i for i, sha in enumerate(in_range)}
    assigned = [sha for p in plan.parts for sha in p.commits]
    for sha in set(assigned) | set(plan.left_on_source):
        _require(sha in position, f"commit {sha} is not in base..source")
    overlap = set(assigned) & set(plan.left_on_source)
    _require(not overlap, f"commits both assigned and left on source: {sorted(overlap)}")
    missing = [sha for sha in in_range if sha not in set(assigned) | set(plan.left_on_source)]
    _require(not missing, f"source commits not assigned to any part or left_on_source: {missing}")
    for part in plan.parts:
        ordered = sorted(part.commits, key=position.__getitem__)
        _require(list(part.commits) == ordered, f"part `{part.id}` commits must be in source order")
        result = subprocess.run(["git", "check-ref-format", "--branch", part.branch], capture_output=True)
        _require(result.returncode == 0, f"part `{part.id}` branch `{part.branch}` is not a valid branch name")


def shared_commits(plan: Plan) -> set[str]:
    seen: dict[str, int] = {}
    for part in plan.parts:
        for sha in part.commits:
            seen[sha] = seen.get(sha, 0) + 1
    return {sha for sha, count in seen.items() if count > 1}


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
        "| # | Part | Branch | Base | Depends on | Commits | Files |",
        "|---|---|---|---|---|---|---|",
    ]
    shared = shared_commits(plan)
    for index, part in enumerate(order, 1):
        files = sorted({f for sha in part.commits for f in commit_files(repo, sha)})
        commits = ", ".join(sha[:9] + ("*" if sha in shared else "") for sha in part.commits)
        deps = ", ".join(part.depends_on) or "—"
        lines.append(
            f"| {index} | {part.id}: {part.title} | `{part.branch}` | `{base_label(plan, part)}` "
            f"| {deps} | {commits} | {', '.join(files)} |"
        )
    if shared:
        lines.append("")
        lines.append("`*` commit shared by several parts: split by hunk, manual reconstruction.")
    if plan.left_on_source:
        lines.append("")
        lines.append("Left on source: " + ", ".join(sha[:9] for sha in plan.left_on_source))
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


def closure(plan: Plan, part: Part) -> list[str]:
    seen: set[str] = set()
    stack = [part.id]
    while stack:
        current = stack.pop()
        if current not in seen:
            seen.add(current)
            stack.extend(plan.part(current).depends_on)
    return sorted(seen)


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
        entry.update({"alone": "clean" if alone else "conflict", "with_deps": "clean" if with_deps else "conflict", "overlaps": overlaps})
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
    if len(parents) >= 2:
        _require(explicit is not None, f"part `{part.id}` has several open parents; pass --onto <merge commit>")
        onto = rev(repo, explicit or "")
        merge_parents = set(git(repo, "rev-list", "--parents", "-n", "1", onto).split()[1:])
        tips = {rev(repo, f"refs/heads/{p.branch}") for p in parents}
        _require(merge_parents == tips, f"--onto must be a merge of exactly the current tips of {[p.branch for p in parents]}")
        return onto
    if len(parents) == 1:
        candidates = [rev(repo, f"refs/heads/{parents[0].branch}")]
    else:
        candidates = [rev(repo, plan.base_ref), plan.base_sha]
    if explicit is not None:
        onto = rev(repo, explicit)
        _require(onto in candidates, f"--onto {explicit} is not the expected base of `{part.id}` ({base_label(plan, part)})")
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
    return {"status": "recorded", "part": part.id, "onto": raw["onto"], "tip": raw["tip"], "pr": raw["pr"], "landed": raw["landed"]}


def restack_commands(plan: Plan, part: Part, parents: list[Part], target: str) -> list[str]:
    if len(parents) <= 1:
        return [f"git rebase --onto {target} {part.onto} {part.branch}"]
    names = ", ".join(p.id for p in parents)
    commands = [
        f"git switch --detach refs/heads/{parents[0].branch}",
        f'git merge --no-ff -m "chore: integrate {names} for {part.id}" '
        + " ".join(f"refs/heads/{p.branch}" for p in parents[1:]),
    ]
    if plan.multi_parent == "merge":
        commands.append(f"git branch -f {part.branch}--base HEAD")
    commands.append(f"git rebase --onto HEAD {part.onto} {part.branch}")
    return commands


def cmd_restack(repo: Path, plan: Plan) -> dict:
    """List the parts whose base moved, in dependency order.

    A part with open parents is stale when a parent tip moved or a parent is
    itself stale. A part whose parents all landed (or that never had any) is
    stale only when its recorded base is no longer in the base branch, which is
    what a squash-merged parent leaves behind; restack never chases the base
    branch otherwise (that is rebase-origin-main's job).
    """
    base_now = rev(repo, plan.base_ref)
    stale: set[str] = set()
    steps = []
    for part in topo_order(plan):
        if part.landed:
            continue
        _require(part.onto is not None and part.tip is not None, f"part `{part.id}` was never recorded")
        assert part.onto is not None
        tip_now = rev(repo, f"refs/heads/{part.branch}")
        _require(tip_now != "", f"branch `{part.branch}` is missing")
        _require(is_ancestor(repo, part.onto, tip_now), f"`{part.branch}` no longer contains its recorded base; restack it by hand")
        parents = open_parents(plan, part)
        landed = [plan.part(d) for d in part.depends_on if plan.part(d).landed]
        if not parents:
            moved = not is_ancestor(repo, part.onto, base_now)
        elif len(parents) == 1:
            moved = part.onto != rev(repo, f"refs/heads/{parents[0].branch}")
        else:
            merged = set(git(repo, "rev-list", "--parents", "-n", "1", part.onto).split()[1:])
            moved = merged != {rev(repo, f"refs/heads/{p.branch}") for p in parents}
        if not (moved or any(p.id in stale for p in parents)):
            continue
        stale.add(part.id)
        target = plan.base_ref if not parents else base_label(plan, part)
        step: dict[str, object] = {
            "part": part.id,
            "branch": part.branch,
            "onto_ref": target,
            "old_onto": part.onto,
            "old_tip": tip_now,
            "commands": restack_commands(plan, part, parents, target),
        }
        published = part.pr is not None and (len(parents) <= 1 or plan.multi_parent == "merge")
        if published:
            step["push"] = (
                f"git push --force-with-lease=refs/heads/{part.branch}:{tip_now} "
                f"origin refs/heads/{part.branch}:refs/heads/{part.branch}"
            )
            if len(parents) >= 2:
                integration = f"{part.branch}--base"
                step["push_base"] = (
                    f"git push --force-with-lease=refs/heads/{integration}:{part.onto} "
                    f"origin refs/heads/{integration}:refs/heads/{integration}"
                )
            elif landed:
                new_base = parents[0].branch if parents else plan.github_base
                step["retarget"] = f"gh pr edit {part.pr} --base {new_base}"
        steps.append(step)
    return {"status": "stale" if steps else "current", "base": base_now, "steps": steps}


def cmd_stack(plan: Plan, part_id: str) -> str:
    part = plan.part(part_id)
    order = [p for p in topo_order(plan)]
    index = order.index(part) + 1
    position = f"{index}/{len(order)}"
    parents = open_parents(plan, part)
    for parent in parents:
        _require(parent.pr is not None, f"parent `{parent.id}` has no PR number recorded yet")
    _require(len(parents) <= 1 or plan.multi_parent == "merge", f"part `{part.id}` waits for {[p.id for p in parents]} to land (multi_parent=wait)")
    if not part.depends_on and not children(plan, part.id) and shape(plan) == "parallel":
        line = f"- **Stack**: independent part {position} of the `{plan.source_branch}` split; base `{plan.github_base}`; merges in any order."
        return line
    if not parents:
        base = f"base `{plan.github_base}`"
        dependency = "no open dependency"
    elif len(parents) == 1:
        base = f"base `{parents[0].branch}`"
        dependency = f"depends on #{parents[0].pr}; merge after it"
    else:
        base = f"base `{part.branch}--base` (merge of {', '.join(f'#{p.pr}' for p in parents)})"
        dependency = "depends on " + " and ".join(f"#{p.pr}" for p in parents) + "; merge after all of them, never into the integration base"
    landed = [plan.part(d) for d in part.depends_on if plan.part(d).landed]
    if landed:
        dependency += "; already landed: " + ", ".join(f"#{p.pr}" for p in landed)
    line = f"- **Stack**: part {position} of the `{plan.source_branch}` split ({shape(plan)}); {base}; {dependency}."
    stacked = [c for c in children(plan, part.id) if c.pr is not None]
    if stacked:
        line += " Stacked on this: " + ", ".join(f"#{c.pr}" for c in stacked) + "."
    return line


def cmd_coverage(repo: Path, plan: Plan) -> dict:
    state = plan.base_sha
    for part in topo_order(plan):
        _require(part.tip is not None, f"part `{part.id}` was never recorded")
        tip = rev(repo, f"refs/heads/{part.branch}")
        _require(is_ancestor(repo, plan.base_sha, tip), f"`{part.branch}` does not contain base.sha; run coverage before restacking")
        tree = merge_tree(repo, state, tip, None)
        _require(tree is not None, f"merging `{part.branch}` into the union conflicts")
        assert tree is not None
        state = synthetic_commit(repo, tree, [state, tip], f"union {part.id}")
    diff = git(repo, "diff", "--name-only", state, plan.source_sha)
    paths = diff.splitlines() if diff else []
    if paths and not plan.left_on_source:
        return {"status": "fail", "differs": paths}
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
