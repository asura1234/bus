#!/usr/bin/env python3
"""Round prologue for /review-plan: combines the prerequisite gate, lane, and round bookkeeping."""

from __future__ import annotations

import argparse
import difflib
import re
import sys
from pathlib import Path

from review_round_support import (
    REPO_ROOT,
    STATUS_RE,
    current_branch,
    detect_branch_plan_docs,
    prereq_failures,
    triage_ledgers,
)


# Lane ownership and round claiming share one implementation with /review-pr, living in the repo-root
# cli_extensions/. Take them from the real owner instead of relaying through
# review_round_support — a pass-through re-export would make "who owns these symbols" unclear again.
sys.path.insert(0, str(REPO_ROOT / "cli_extensions"))
from review_round_common import (  # noqa: E402
    bare_call_ambiguous_lanes,
    claim_bare_round,
    lanes_with_history,
    rel,
)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("plan", nargs="?", default=None)
    parser.add_argument("--reviewer", default=None)
    parser.add_argument("--devils-advocate", action="store_true", dest="devils_advocate")
    # --check: run only the structure/status gate; read-only, creates no round, writes no .last-plan.
    #   Serves both the plan author's self-check before setting create-plan-complete and
    #   commit/publish re-validating a finalized plan after review-plan-complete; a real review
    #   round still rejects the completed status.
    parser.add_argument("--check", action="store_true")
    try:
        args = parser.parse_args(argv[1:])
    except SystemExit:
        print(
            "usage: review_round.py [<plan.md>] [--reviewer <lane>] [--devils-advocate] [--check]",
            file=sys.stderr,
        )
        return 2

    branch = current_branch()
    branch_slug = re.sub(r"[^A-Za-z0-9_-]", "-", branch)
    branch_dir = REPO_ROOT / "temp" / "review-plan" / branch_slug
    pointer = branch_dir / ".last-plan"

    notes: list[str] = []
    # The plan file may be omitted: one branch, one plan. When omitted, reuse the plan this branch last
    # reviewed, so the user can repeatedly bare-invoke `/review-plan` until convergence without restating the path.
    if args.plan is not None:
        plan_path = Path(args.plan)
        # An explicit relative path resolves against CWD first and falls back to REPO_ROOT, so calls from outside the repo work
        if not plan_path.is_file() and not plan_path.is_absolute():
            anchored = REPO_ROOT / args.plan
            if anchored.is_file():
                plan_path = anchored
        if not plan_path.is_file():
            print(f"error: {plan_path} not found", file=sys.stderr)
            return 2
    elif pointer.is_file():
        # The pointer stores a repo-relative path; restore it against REPO_ROOT, independent of the caller's working directory
        recorded = pointer.read_text().strip()
        plan_path = Path(recorded) if Path(recorded).is_absolute() else REPO_ROOT / recorded
        if not plan_path.is_file():
            print(
                f"error: 本分支记录的计划 {plan_path} 已不存在；请显式指定 /review-plan <doc>",
                file=sys.stderr,
            )
            return 2
        notes.append(f"未指定计划，沿用本分支上次审查的计划 {rel(plan_path)}")
    else:
        candidates = detect_branch_plan_docs()
        if len(candidates) == 1:
            plan_path = candidates[0]
            notes.append(f"未指定计划，自动选中本分支改动的唯一计划文档 {rel(plan_path)}")
        else:
            print("FAIL")
            if not candidates:
                print("- No plan was specified and no changed plans/*.md file was detected; pass /review-plan <doc> explicitly.")
            else:
                print("- 本分支改动了多个计划文档，无法自动选择；请显式指定 /review-plan <doc>：")
                for c in candidates:
                    print(f"  - {rel(c)}")
            return 1

    # Lane selection: a bare call (no --reviewer) continues the stable 'default' lane and claims the round
    # concurrency-safely — see claim_bare_round below (when 'default' is held concurrently it is isolated into
    # default-2/3…, without overwriting each other). Continuing a round does not depend on the agent remembering
    # any id, and is unaffected by --devils-advocate (lane is independent of posture). Here we first validate a
    # named lane: an empty string after sanitizing fails fast (illegal characters become '-' and never empty it,
    # so this only fires when --reviewer is passed an empty string), avoiding a silent fallback to default that
    # would cross-talk with bare calls.
    default_lane = args.reviewer is None
    reviewer: str | None = None
    if not default_lane:
        reviewer = re.sub(r"[^A-Za-z0-9_-]", "-", args.reviewer)
        if not reviewer:
            print(
                "error: --reviewer 清洗后为空；请提供含 [A-Za-z0-9_-] 的有效 lane 名",
                file=sys.stderr,
            )
            return 2

    text = plan_path.read_text()
    failures = prereq_failures(text, check_only=args.check)
    if failures:
        print("FAIL")
        for f in failures:
            print(f"- {f}")
        return 1

    # --check: return as soon as the structure/status gate passes, with no round bookkeeping (no .last-plan, no round directory).
    if args.check:
        print("PASS: 计划满足 /review-plan 前提门禁（模板 / 状态 / 决策 / 目标 / 实施步骤结构）")
        return 0

    # Once the plan passes the prerequisite checks, record it as this branch's "last reviewed plan" for later bare calls
    branch_dir.mkdir(parents=True, exist_ok=True)
    pointer.write_text(rel(plan_path) + "\n")

    status = STATUS_RE.search(text).group(1)

    lane_root = branch_dir / plan_path.stem
    if default_lane:
        # Ownership safety: a bare call continues automatically only with "no history" or "only default has
        # history". Once a named lane exists or multiple lanes have history (multi-reviewer), a bare call cannot
        # tell which to continue — FAIL and require an explicit --reviewer, instead of silently falling back to
        # default (which would lose cross-round ownership and let a reviewer read someone else's history).
        prior_lanes = lanes_with_history(lane_root)
        ambiguous = bare_call_ambiguous_lanes(prior_lanes)
        if ambiguous:
            print("FAIL")
            print(
                "- 本计划已有多条 / 具名 reviewer lane，裸调用无法确定要续哪条；"
                "请显式指定 --reviewer <lane> 续审你自己的 lane。已存在的 lane："
            )
            for ln in prior_lanes:
                print(f"  - {ln}")
            print(
                "  （自动 default / default-2 仅用于单 reviewer 或首轮并发启动，"
                "不支持多 lane 跨轮归属；跨轮稳定续审必须显式传同一个 --reviewer <name>。）"
            )
            return 1
        reviewer, current, completed = claim_bare_round(lane_root)
        if reviewer == "default":
            notes.append(
                "未指定 --reviewer，续用本分支本计划的 'default' lane（裸调即自动续轮、先核销前轮）。"
                "并行的多个独立 reviewer 各自传一个不同的 --reviewer <lane>，可获得跨轮稳定隔离。"
            )
        else:
            notes.append(
                f"'default' lane 正被另一并发实例占用，已自动隔离到 '{reviewer}'，与其它实例互不覆盖"
                "（本轮模式见 MODE）。并行多视角若需跨轮稳定续审，请给每个实例显式传 --reviewer <name>。"
            )
        state_dir = lane_root / reviewer
    else:
        state_dir = lane_root / reviewer
        state_dir.mkdir(parents=True, exist_ok=True)
        round_dirs = sorted(d for d in state_dir.glob("round-*") if d.is_dir())
        completed = [d for d in round_dirs if (d / "review.md").is_file()]
        aborted = [d for d in round_dirs if not (d / "review.md").is_file()]
        if aborted:
            current = aborted[-1]
            notes.append(f"复用上次未完成的轮次目录 {current.name}")
        else:
            current = state_dir / f"round-{len(completed) + 1:02d}"
            current.mkdir(exist_ok=True)

    if status == "review-plan-in-progress" and not completed:
        notes.append(
            f"状态为 review-plan-in-progress 但 lane '{reviewer}' 无轮次历史"
            "（temp 被清理 / 新 worktree / 新 reviewer）——本 lane 从第 1 轮开始"
        )
    round_n = int(current.name.split("-")[1])

    (current / "plan-snapshot.md").write_text(text)

    diff_path: Path | None = None
    if completed:
        prev_snapshot = completed[-1] / "plan-snapshot.md"
        if prev_snapshot.is_file():
            diff_lines = list(
                difflib.unified_diff(
                    prev_snapshot.read_text().splitlines(keepends=True),
                    text.splitlines(keepends=True),
                    fromfile=f"{completed[-1].name}/plan-snapshot.md",
                    tofile=f"{current.name}/plan-snapshot.md",
                )
            )
            diff_path = current / "plan-diff.patch"
            diff_path.write_text("".join(diff_lines))
            if not diff_lines:
                notes.append("计划自上一轮审查以来无任何改动")
        else:
            notes.append(f"{completed[-1].name} 缺少 plan-snapshot.md，本轮无增量 diff")

    print(f"ROUND={round_n}")
    # MODE is decided by "does this lane have a completed round", not by the round number: a leftover aborted
    # high-numbered round directory must not make the mode incremental without any PREV_REVIEWS
    print(f"MODE={'incremental' if completed else 'full'}")
    print(f"POSTURE={'devils-advocate' if args.devils_advocate else 'standard'}")
    print(f"REVIEWER={reviewer}")
    print(f"BRANCH={branch}")
    print(f"PLAN={rel(plan_path)}")
    print(f"STATE_DIR={rel(state_dir)}")
    print(f"SNAPSHOT={rel(current / 'plan-snapshot.md')}")
    print(f"DIFF={rel(diff_path) if diff_path else 'none'}")
    prev_reviews = ",".join(rel(d / "review.md") for d in completed)
    print(f"PREV_REVIEWS={prev_reviews if prev_reviews else 'none'}")
    triage = triage_ledgers(branch_slug)
    print(f"TRIAGE_LEDGER={','.join(rel(t) for t in triage) if triage else 'none'}")
    for note in notes:
        print(f"NOTE={note}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
