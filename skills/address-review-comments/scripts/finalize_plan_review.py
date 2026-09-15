#!/usr/bin/env python3
"""Author-side plan-review finalization bound to one plan hash and explicit Ready lane rounds.

Only an explicit room Orchestrator assignment invokes `finalize`. The helper never reads a
lane for judgment, never chooses a next workflow action, and writes only the plan status.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


REPO_ROOT = Path(__file__).resolve().parents[3]
CLI_EXTENSIONS = REPO_ROOT / "cli_extensions"
if str(CLI_EXTENSIONS) not in sys.path:
    sys.path.insert(0, str(CLI_EXTENSIONS))

from review_artifact_parser import parse_review_artifact  # noqa: E402
from review_artifact_types import ReviewArtifactError  # noqa: E402


STATUS_RE = re.compile(r"^\*\*状态\*\*：[ \t]*(\S+)[ \t]*$", re.MULTILINE)
LANE_RE = re.compile(r"^(?P<lane>[A-Za-z0-9_-]+):(?P<round>[1-9][0-9]*)$")
ROUND_DIR_RE = re.compile(r"^round-(?P<round>[0-9]+)$")
READY_VERDICT = "可执行（Ready）"
IN_PROGRESS = "review-plan-in-progress"
COMPLETE = "review-plan-complete"


class FinalizationError(ValueError):
    """Finalization evidence is missing, stale, mixed, duplicated, or not Ready."""


@dataclass(frozen=True)
class LaneRound:
    lane: str
    round_number: int


def plan_hash(text: str) -> str:
    """Hash plan content; the status value is workflow bookkeeping, not reviewed content."""
    if len(STATUS_RE.findall(text)) != 1:
        raise FinalizationError("plan must contain exactly one **状态** field")
    normalized = STATUS_RE.sub("**状态**：<status>", text, count=1)
    return hashlib.sha256(normalized.encode("utf-8")).hexdigest()


def parse_lane_rounds(values: Sequence[str]) -> tuple[LaneRound, ...]:
    if not values:
        raise FinalizationError("at least one --lane <lane>:<round> is required")
    lanes: list[LaneRound] = []
    seen: set[str] = set()
    for value in values:
        match = LANE_RE.fullmatch(value)
        if match is None:
            raise FinalizationError(f"invalid lane round: {value}")
        lane = match.group("lane")
        if lane in seen:
            raise FinalizationError(f"duplicate lane: {lane}")
        seen.add(lane)
        lanes.append(LaneRound(lane, int(match.group("round"))))
    return tuple(lanes)


def _same_plan(target: str, plan: Path) -> bool:
    target_path = Path(target)
    if not target_path.is_absolute():
        target_path = REPO_ROOT / target_path
    return target_path.resolve() == plan.resolve()


def _lane_failures(
    plan: Path,
    review_root: Path,
    lane: LaneRound,
) -> tuple[list[str], str | None]:
    lane_dir = review_root / lane.lane
    round_dir = lane_dir / f"round-{lane.round_number:02d}"
    review = round_dir / "review.md"
    snapshot = round_dir / "plan-snapshot.md"
    label = f"{lane.lane}:{lane.round_number}"
    if not review.is_file() or not snapshot.is_file():
        return [f"{label}: missing review.md or plan-snapshot.md in {round_dir}"], None
    failures: list[str] = []
    later = sorted(
        entry.name
        for entry in lane_dir.iterdir()
        if entry.is_dir()
        and (match := ROUND_DIR_RE.fullmatch(entry.name)) is not None
        and int(match.group("round")) > lane.round_number
        and (entry / "review.md").is_file()
    )
    if later:
        failures.append(f"{label}: stale round; later completed rounds exist: {', '.join(later)}")
    try:
        artifact = parse_review_artifact(review)
    except ReviewArtifactError as error:
        return [*failures, f"{label}: {error}"], None
    if artifact.mode != "plan" or artifact.lane != lane.lane:
        failures.append(f"{label}: review artifact is not this plan-review lane")
    if not _same_plan(artifact.target["plan"], plan):
        failures.append(f"{label}: review artifact targets another plan")
    if artifact.verdict != READY_VERDICT or artifact.substantive != "无。":
        failures.append(f"{label}: round is not a finding-free Ready verdict")
    try:
        snapshot_hash = plan_hash(snapshot.read_text(encoding="utf-8"))
    except (FinalizationError, OSError, UnicodeError) as error:
        return [*failures, f"{label}: unreadable plan snapshot: {error}"], None
    return failures, snapshot_hash


def finalize(
    plan: Path,
    review_root: Path,
    expected_hash: str,
    lanes: Sequence[LaneRound],
) -> list[str]:
    text = plan.read_text(encoding="utf-8")
    current = plan_hash(text)
    failures: list[str] = []
    if current != expected_hash:
        failures.append(f"stale plan hash: current {current} != expected {expected_hash}")
    status = STATUS_RE.search(text).group(1)
    if status != IN_PROGRESS:
        failures.append(f"plan status must be {IN_PROGRESS}, found {status}")
    snapshot_hashes: dict[str, str] = {}
    for lane in lanes:
        lane_failures, snapshot_hash = _lane_failures(plan, review_root, lane)
        failures.extend(lane_failures)
        if snapshot_hash is not None:
            snapshot_hashes[lane.lane] = snapshot_hash
    if len(set(snapshot_hashes.values())) > 1:
        failures.append(
            "mixed plan hashes across lanes: "
            + ", ".join(f"{lane}={value}" for lane, value in snapshot_hashes.items())
        )
    failures.extend(
        f"{lane}: reviewed plan hash {value} is stale against current {current}"
        for lane, value in snapshot_hashes.items()
        if value != current
    )
    if failures:
        raise FinalizationError("\n".join(failures))

    updated = STATUS_RE.sub(f"**状态**：{COMPLETE}", text, count=1)
    handle, temporary = tempfile.mkstemp(dir=plan.parent, prefix=f".{plan.name}.")
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            stream.write(updated)
        os.replace(temporary, plan)
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise
    return [
        f"FINALIZED={COMPLETE}",
        f"PLAN={plan}",
        f"PLAN_HASH={current}",
        "LANES=" + ",".join(f"{lane.lane}:{lane.round_number}" for lane in lanes),
    ]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Hash-bound plan-review finalization")
    commands = parser.add_subparsers(dest="command", required=True)
    hash_command = commands.add_parser("hash")
    hash_command.add_argument("--plan", type=Path, required=True)
    finalize_command = commands.add_parser("finalize")
    finalize_command.add_argument("--plan", type=Path, required=True)
    finalize_command.add_argument("--review-root", type=Path, required=True)
    finalize_command.add_argument("--expected-plan-hash", required=True)
    finalize_command.add_argument("--lane", action="append", default=[])
    args = parser.parse_args(argv)
    try:
        if args.command == "hash":
            print(f"PLAN_HASH={plan_hash(args.plan.read_text(encoding='utf-8'))}")
            return 0
        receipt = finalize(
            args.plan,
            args.review_root,
            args.expected_plan_hash,
            parse_lane_rounds(args.lane),
        )
    except (FinalizationError, OSError, UnicodeError) as error:
        print("FAIL")
        for line in str(error).splitlines():
            print(f"- {line}")
        return 1
    for line in receipt:
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
