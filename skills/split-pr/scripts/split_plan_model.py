"""Plan model for split_plan.py: schema, validation and graph values derived from the plan."""

from __future__ import annotations

import json
import os
import re
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
    left_reasons: tuple[str, ...]

    def part(self, part_id: str) -> Part:
        for part in self.parts:
            if part.id == part_id:
                return part
        raise PlanError(f"unknown part `{part_id}`")

    @property
    def github_base(self) -> str:
        return self.base_ref.removeprefix("origin/")


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
        _require(
            isinstance(value["sha"], str) and SHA_RE.match(value["sha"]) is not None, f"`{name}.sha` must be a full SHA"
        )
    _require(isinstance(source["branch"], str) and source["branch"] != "", "`source.branch` is required")
    _require(
        isinstance(base["ref"], str) and base["ref"].startswith("origin/") and base["ref"] != "origin/",
        "`base.ref` must be an explicit origin/<branch> ref",
    )
    _require(data["multi_parent"] in MULTI_PARENT_POLICIES, f"`multi_parent` must be one of {MULTI_PARENT_POLICIES}")
    _require(isinstance(data["parts"], list) and len(data["parts"]) >= 2, "a split needs at least two parts")
    entries = data["left_on_source"]
    _require(
        isinstance(entries, list)
        and all(
            isinstance(e, dict)
            and set(e) == {"sha", "reason"}
            and isinstance(e["sha"], str)
            and SHA_RE.match(e["sha"]) is not None
            and isinstance(e["reason"], str)
            and e["reason"].strip() != ""
            for e in entries
        ),
        "`left_on_source` must be a list of {sha, reason} with a full SHA and a non-empty reason",
    )
    left = [e["sha"] for e in entries]
    _require(len(set(left)) == len(left), "`left_on_source` has duplicates")

    parts: list[Part] = []
    for index, raw in enumerate(data["parts"]):
        where = f"parts[{index}]"
        _require(isinstance(raw, dict) and set(raw) == PART_KEYS, f"{where} keys must be exactly {sorted(PART_KEYS)}")
        _require(
            isinstance(raw["id"], str) and ID_RE.match(raw["id"]) is not None, f"{where}.id must match {ID_RE.pattern}"
        )
        _require(isinstance(raw["branch"], str) and raw["branch"] != "", f"{where}.branch is required")
        _require(isinstance(raw["title"], str) and raw["title"].strip() != "", f"{where}.title is required")
        deps, commits = raw["depends_on"], raw["commits"]
        _require(
            isinstance(deps, list) and all(isinstance(d, str) for d in deps),
            f"{where}.depends_on must be a list of ids",
        )
        _require(len(set(deps)) == len(deps), f"{where}.depends_on has duplicates")
        _require(isinstance(commits, list) and commits != [], f"{where}.commits must be a non-empty list")
        _require(all(isinstance(s, str) and SHA_RE.match(s) for s in commits), f"{where}.commits must be full SHAs")
        _require(len(set(commits)) == len(commits), f"{where}.commits has duplicates")
        for key in ("onto", "tip"):
            value = raw[key]
            _require(
                value is None or (isinstance(value, str) and SHA_RE.match(value) is not None),
                f"{where}.{key} must be null or a full SHA",
            )
        _require(raw["tip"] is None or raw["onto"] is not None, f"{where}.tip requires onto")
        pr = raw["pr"]
        _require(
            pr is None or (isinstance(pr, int) and not isinstance(pr, bool) and pr > 0),
            f"{where}.pr must be null or a positive integer",
        )
        _require(isinstance(raw["landed"], bool), f"{where}.landed must be a boolean")
        _require(not raw["landed"] or pr is not None, f"{where} cannot be landed without a PR")
        parts.append(
            Part(
                raw["id"],
                raw["branch"],
                raw["title"],
                tuple(deps),
                tuple(commits),
                raw["onto"],
                raw["tip"],
                pr,
                raw["landed"],
            )
        )

    ids = [p.id for p in parts]
    _require(len(set(ids)) == len(ids), "part ids must be unique")
    branches = [p.branch for p in parts]
    _require(len(set(branches)) == len(branches), "part branches must be unique")
    prs = [p.pr for p in parts if p.pr is not None]
    _require(len(set(prs)) == len(prs), "part PR numbers must be unique")
    for part in parts:
        _require(
            part.branch not in {source["branch"], base["ref"], base["ref"].removeprefix("origin/")},
            f"part `{part.id}` reuses the source or base branch name",
        )
        for dep in part.depends_on:
            _require(dep in ids, f"part `{part.id}` depends on unknown part `{dep}`")
            _require(dep != part.id, f"part `{part.id}` depends on itself")
    plan = Plan(
        source["branch"],
        source["sha"],
        base["ref"],
        base["sha"],
        data["multi_parent"],
        tuple(parts),
        tuple(left),
        tuple(e["reason"] for e in entries),
    )
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


def integrates_base(plan: Plan, part: Part) -> bool:
    """A partial fan-in: a parent landed while another is still open.

    The open parent still sits on the old base and lacks the landed parent's
    code, so the part must integrate the current base alongside it.
    """
    parents = open_parents(plan, part)
    return bool(parents) and len(parents) < len(part.depends_on)


def integration_refs(plan: Plan, part: Part) -> list[str]:
    """Refs the part's base merges; a single ref means no integration merge."""
    refs = [p.branch for p in open_parents(plan, part)]
    return [plan.base_ref, *refs] if integrates_base(plan, part) else refs


def base_label(plan: Plan, part: Part) -> str:
    refs = integration_refs(plan, part)
    if not refs:
        return plan.base_ref
    if len(refs) == 1:
        return refs[0]
    return "merge(" + ", ".join(refs) + ")"


def shared_commits(plan: Plan) -> set[str]:
    seen: dict[str, int] = {}
    for part in plan.parts:
        for sha in part.commits:
            seen[sha] = seen.get(sha, 0) + 1
    return {sha for sha, count in seen.items() if count > 1}


def closure(plan: Plan, part: Part) -> list[str]:
    seen: set[str] = set()
    stack = [part.id]
    while stack:
        current = stack.pop()
        if current not in seen:
            seen.add(current)
            stack.extend(plan.part(current).depends_on)
    return sorted(seen)
