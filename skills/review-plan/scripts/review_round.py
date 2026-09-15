#!/usr/bin/env python3
"""Round prologue for /review-plan：组合前提门禁、lane 与 round bookkeeping。"""

from __future__ import annotations

import argparse
import difflib
import re
import sys
from pathlib import Path

from review_round_support import (
    REPO_ROOT,
    STATUS_RE,
    bare_call_ambiguous_lanes,
    claim_bare_round,
    current_branch,
    detect_branch_plan_docs,
    lanes_with_history,
    prereq_failures,
    rel,
    triage_ledgers,
)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("plan", nargs="?", default=None)
    parser.add_argument("--reviewer", default=None)
    parser.add_argument("--devils-advocate", action="store_true", dest="devils_advocate")
    # --check：只跑结构/状态门禁，read-only、不建轮次、不写 .last-plan。
    #   既供 create-plan 在置 create-plan-complete 前自检，也供 commit/publish 在
    #   review-plan-complete 后复验 finalized plan；真正的 review round 仍拒绝 completed 状态。
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
    # 计划文件可省略：一个分支一个计划，省略时沿用本分支上次审查的计划，让用户可以反复
    # 裸调用 `/review-plan` 直到收敛，无需每次重述路径。
    if args.plan is not None:
        plan_path = Path(args.plan)
        # 显式相对路径优先按 CWD 解析，找不到时回退 REPO_ROOT，兼容从仓库外目录调用
        if not plan_path.is_file() and not plan_path.is_absolute():
            anchored = REPO_ROOT / args.plan
            if anchored.is_file():
                plan_path = anchored
        if not plan_path.is_file():
            print(f"error: {plan_path} not found", file=sys.stderr)
            return 2
    elif pointer.is_file():
        # 指针存的是仓库相对路径，按 REPO_ROOT 还原，不依赖调用时的工作目录
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

    # lane 选择：裸调用（无 --reviewer）续用稳定 'default' lane，并发安全地认领轮次——见下方
    # claim_bare_round（'default' 被并发占用时自动隔离到 default-2/3…，互不覆盖）。续轮不依赖
    # agent 记住任何 id，也不受 --devils-advocate 与否影响（lane 与姿态无关）。这里先处理具名
    # lane 的名字校验：清洗后为空串即 fail-fast（非法字符会被替换成 '-' 不会变空，故仅当 --reviewer
    # 传了空串时触发），避免静默落回 default 与裸调用串台。
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

    # --check：结构/状态门禁通过即返回，不做任何轮次 bookkeeping（不写 .last-plan、不建 round 目录）。
    if args.check:
        print("PASS: 计划满足 /review-plan 前提门禁（模板 / 状态 / 决策 / 目标 / 实施步骤结构）")
        return 0

    # 计划通过前提校验后，记录为本分支的「上次审查计划」，供后续裸调用沿用
    branch_dir.mkdir(parents=True, exist_ok=True)
    pointer.write_text(rel(plan_path) + "\n")

    status = STATUS_RE.search(text).group(1)

    lane_root = branch_dir / plan_path.stem
    if default_lane:
        # 归属安全：裸调用只在「无历史」或「仅 default 有历史」时自动续轮。一旦存在具名 lane 或
        # 多条 lane 有历史（多 reviewer 场景），裸调用无法确定要续哪条——FAIL 要求显式 --reviewer，
        # 不再静默落回 default（那会丢失跨轮归属、让 reviewer 读串别人的历史）。
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
    # MODE 以「本 lane 是否存在已完成轮」为准，而非 round 编号：残留的 aborted 高编号
    # 轮次目录不应在没有任何 PREV_REVIEWS 的情况下把模式判成 incremental
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
