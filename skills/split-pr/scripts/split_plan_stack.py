"""Restack steps and the Stack summary line for split_plan.py."""

from __future__ import annotations

from pathlib import Path

from split_plan_git import git, is_ancestor, rev
from split_plan_model import (
    Part,
    Plan,
    _require,
    base_label,
    children,
    integrates_base,
    integration_refs,
    open_parents,
    shape,
    topo_order,
)


def restack_commands(plan: Plan, part: Part, target: str) -> list[str]:
    parents = open_parents(plan, part)
    refs = [ref if ref == plan.base_ref else f"refs/heads/{ref}" for ref in integration_refs(plan, part)]
    if len(refs) <= 1:
        return [f"git rebase --onto {target} {part.onto} {part.branch}"]
    names = ", ".join(([plan.github_base] if integrates_base(plan, part) else []) + [p.id for p in parents])
    commands = [
        f"git switch --detach {refs[0]}",
        f'git merge --no-ff -m "chore: integrate {names} for {part.id}" ' + " ".join(refs[1:]),
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
        _require(
            is_ancestor(repo, part.onto, tip_now),
            f"`{part.branch}` no longer contains its recorded base; restack it by hand",
        )
        parents = open_parents(plan, part)
        landed = [plan.part(d) for d in part.depends_on if plan.part(d).landed]
        integrated = len(integration_refs(plan, part)) >= 2
        if not parents:
            moved = not is_ancestor(repo, part.onto, base_now)
        elif not integrated:
            moved = part.onto != rev(repo, f"refs/heads/{parents[0].branch}")
        else:
            merged = set(git(repo, "rev-list", "--parents", "-n", "1", part.onto).split()[1:])
            tips = {rev(repo, f"refs/heads/{p.branch}") for p in parents}
            if landed:
                # The base side only needs to be some base commit that already
                # holds the landed parents; later base movement is rebase-origin-main's job.
                extra = merged - tips
                moved = not (tips <= merged and len(extra) == 1 and is_ancestor(repo, extra.pop(), base_now))
            else:
                moved = merged != tips
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
            "commands": restack_commands(plan, part, target),
        }
        published = part.pr is not None and (not integrated or plan.multi_parent == "merge")
        if published:
            step["push"] = (
                f"git push --force-with-lease=refs/heads/{part.branch}:{tip_now} "
                f"origin refs/heads/{part.branch}:refs/heads/{part.branch}"
            )
            if integrated:
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
    integrated = len(integration_refs(plan, part)) >= 2
    for parent in parents:
        _require(parent.pr is not None, f"parent `{parent.id}` has no PR number recorded yet")
    _require(
        not integrated or plan.multi_parent == "merge",
        f"part `{part.id}` waits for {[p.id for p in parents]} to land (multi_parent=wait)",
    )
    if not part.depends_on and not children(plan, part.id) and shape(plan) == "parallel":
        line = f"- **Stack**: independent part {position} of the `{plan.source_branch}` split; base `{plan.github_base}`; merges in any order."
        return line
    if not parents:
        base = f"base `{plan.github_base}`"
        dependency = "no open dependency"
    elif not integrated:
        base = f"base `{parents[0].branch}`"
        dependency = f"depends on #{parents[0].pr}; merge after it"
    else:
        sources = [f"`{plan.github_base}`"] if integrates_base(plan, part) else []
        sources += [f"#{p.pr}" for p in parents]
        base = f"base `{part.branch}--base` (merge of {', '.join(sources)})"
        dependency = (
            "depends on "
            + " and ".join(f"#{p.pr}" for p in parents)
            + "; merge after all of them, never into the integration base"
        )
    landed = [plan.part(d) for d in part.depends_on if plan.part(d).landed]
    if landed:
        dependency += "; already landed: " + ", ".join(f"#{p.pr}" for p in landed)
    line = f"- **Stack**: part {position} of the `{plan.source_branch}` split ({shape(plan)}); {base}; {dependency}."
    stacked = [c for c in children(plan, part.id) if c.pr is not None]
    if stacked:
        line += " Stacked on this: " + ", ".join(f"#{c.pr}" for c in stacked) + "."
    return line
