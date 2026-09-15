#!/usr/bin/env python3
"""Render the PR goal/non-goal fragment and the review-pr branch goal and non-goal locks."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Sequence


REPO_ROOT = Path(__file__).resolve().parents[3]
CLI_EXTENSIONS = REPO_ROOT / "cli_extensions"
if str(CLI_EXTENSIONS) not in sys.path:
    sys.path.insert(0, str(CLI_EXTENSIONS))

from room_assignment_context import OrchestratedContext, read_assignment_context  # noqa: E402


FENCE_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})")
H2_RE = re.compile(r"^## ([^\r\n]+)$")


def _fence_marker(line: str) -> tuple[str, int] | None:
    match = FENCE_RE.match(line)
    if match is None:
        return None
    marker = match.group(1)
    return marker[0], len(marker)


def _trim_boundary_blank_lines(text: str) -> str:
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    return "\n".join(lines)


def split_h2_sections(text: str) -> list[tuple[str, str]]:
    """Return unfenced H2 sections while preserving each raw Markdown body."""
    sections: list[tuple[str, list[str]]] = []
    current: tuple[str, list[str]] | None = None
    fence: tuple[str, int] | None = None
    for line in text.replace("\r\n", "\n").replace("\r", "\n").splitlines(
        keepends=True
    ):
        stripped = line.rstrip("\n")
        marker = _fence_marker(stripped)
        if fence is None:
            heading = H2_RE.fullmatch(stripped)
            if heading is not None:
                current = (heading.group(1), [])
                sections.append(current)
                continue
            if marker is not None:
                fence = marker
        elif marker is not None and marker[0] == fence[0] and marker[1] >= fence[1]:
            fence = None
        if current is not None:
            current[1].append(line)
    return [(title, "".join(lines)) for title, lines in sections]


def _extract_section(text: str, heading: str, source: str) -> str:
    matches = [body for title, body in split_h2_sections(text) if title == heading]
    if not matches:
        raise ValueError(f"{source} 缺少 `## {heading}`")
    if len(matches) != 1:
        raise ValueError(f"{source} 重复声明 `## {heading}`")
    body = _trim_boundary_blank_lines(matches[0])
    if not body:
        raise ValueError(f"{source} 的 `## {heading}` 为空")
    return body


def extract_plan_section(plan_text: str, heading: str) -> str:
    return _extract_section(plan_text, heading, "计划")


def build_context(plan_texts: Sequence[str]) -> tuple[str, str]:
    if not plan_texts:
        raise ValueError("至少需要一份计划")
    goals = [extract_plan_section(text, "目标") for text in plan_texts]
    non_goals = [extract_plan_section(text, "非目标") for text in plan_texts]
    return "\n\n".join(goals), "\n\n".join(non_goals)


def parse_context(context_text: str) -> tuple[str, str]:
    sections = split_h2_sections(context_text)
    if [title for title, _body in sections] != ["目标", "非目标"]:
        raise ValueError("goal context 必须只含按顺序排列的 `## 目标` 与 `## 非目标`")
    return (
        _extract_section(context_text, "目标", "goal context"),
        _extract_section(context_text, "非目标", "goal context"),
    )


def render_context(goal: str, non_goal: str) -> str:
    return f"## 目标\n\n{goal}\n\n## 非目标\n\n{non_goal}\n"


def locked_context_paths(repo_root: Path, branch: str) -> tuple[Path, Path]:
    branch_slug = re.sub(r"[^A-Za-z0-9_-]", "-", branch)
    lane_root = repo_root / "temp" / "review-pr" / branch_slug
    return lane_root / ".locked-goal", lane_root / ".locked-non-goals"


def _verified_room_brief(path: Path) -> tuple[str, str]:
    context = read_assignment_context(path)
    if not isinstance(context, OrchestratedContext):
        raise ValueError("room assignment context 必须来自 verified TRUSTED_ROOM_ASSIGNMENT_V1")
    return (
        _trim_boundary_blank_lines(context.goal),
        _trim_boundary_blank_lines(context.non_goals),
    )


def prepare_context(
    *,
    branch: str,
    output_path: Path,
    repo_root: Path,
    plan_paths: Sequence[Path] = (),
    goal_path: Path | None = None,
    non_goal_path: Path | None = None,
    assignment_context_path: Path | None = None,
) -> Path:
    if not branch or branch in {"main", "master"}:
        raise ValueError("必须提供非 main/master 的具名 feature branch")
    authored = goal_path is not None or non_goal_path is not None
    if authored and (plan_paths or assignment_context_path is not None):
        raise ValueError("authored goal/non-goal 模式不能与计划或 room assignment context 混用")

    room_brief = (
        _verified_room_brief(assignment_context_path)
        if assignment_context_path is not None
        else None
    )
    if plan_paths:
        goal, non_goal = build_context(
            [path.read_text(encoding="utf-8") for path in plan_paths]
        )
        if room_brief is not None and (goal, non_goal) != room_brief:
            raise ValueError("计划 Goal/Non-goals 与 verified Room Brief 不一致；不得覆盖")
    elif room_brief is not None:
        goal, non_goal = room_brief
    else:
        if goal_path is None or non_goal_path is None:
            raise ValueError("无计划时必须同时提供 --goal-file 与 --non-goal-file")
        goal = _trim_boundary_blank_lines(goal_path.read_text(encoding="utf-8"))
        non_goal = _trim_boundary_blank_lines(non_goal_path.read_text(encoding="utf-8"))
        if not goal or not non_goal:
            raise ValueError("authored goal/non-goal 不得为空")

    locked_goal_path, locked_non_goals_path = locked_context_paths(repo_root, branch)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    locked_goal_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(render_context(goal, non_goal), encoding="utf-8")
    locked_goal_path.write_text(f"{goal}\n", encoding="utf-8")
    locked_non_goals_path.write_text(f"{non_goal}\n", encoding="utf-8")
    return locked_goal_path


def _relative(path: Path, root: Path) -> str:
    try:
        return str(path.resolve().relative_to(root.resolve()))
    except ValueError:
        return str(path.resolve())


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Prepare exact PR goal/non-goal context and review-pr goal/non-goal locks"
    )
    parser.add_argument("--branch", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--plan", type=Path, action="append", default=[])
    parser.add_argument("--goal-file", type=Path)
    parser.add_argument("--non-goal-file", type=Path)
    parser.add_argument("--assignment-context", type=Path)
    args = parser.parse_args(argv)
    try:
        locked_goal_path = prepare_context(
            branch=args.branch,
            output_path=args.output,
            repo_root=REPO_ROOT,
            plan_paths=args.plan,
            goal_path=args.goal_file,
            non_goal_path=args.non_goal_file,
            assignment_context_path=args.assignment_context,
        )
    except (OSError, UnicodeError, ValueError) as error:
        print(f"pr goal context failed: {error}", file=sys.stderr)
        return 1
    _, locked_non_goals_path = locked_context_paths(REPO_ROOT, args.branch)
    print(f"GOAL_CONTEXT_FILE={_relative(args.output, REPO_ROOT)}")
    print(f"LOCKED_GOAL_FILE={_relative(locked_goal_path, REPO_ROOT)}")
    print(f"LOCKED_NON_GOALS_FILE={_relative(locked_non_goals_path, REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
