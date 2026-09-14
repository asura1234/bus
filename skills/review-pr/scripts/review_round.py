#!/usr/bin/env python3
"""Round prologue for /review-pr: deterministic prereq check + round bookkeeping.

No agent judgment, and NO repo mutation: the reviewer is read-only over the code;
the only writes are this script's own round artifacts under temp/review-pr/.

What it does:

  1. Prereq (前提不满足 → FAIL):
     - inside a git work tree, on a named branch that is not main/master
     - base ref (default origin/master) resolvable
     - committed non-plan diff is non-empty (nothing to review otherwise); `plans/**`
       is excluded because plans are locked review context, not code-review targets
     - when no plan is associated, a developer-supplied branch-level locked goal exists

  2. Round bookkeeping under temp/review-pr/<branch>/<reviewer>/:
     - each reviewer gets an isolated lane — multi-reviewer lanes must not collide
       on round dirs, and lanes stay independent so reviewers don't anchor on each
       other's findings
     - --reviewer omitted → the stable 'default' lane, so repeated bare
       `/review-pr` on a branch continues the SAME lane and accumulates rounds
       (one branch = one purpose). Concurrent bare invocations are handled
       safely by claim_bare_round: if 'default' is in-flight (another live
       instance), the call atomically bumps to default-2 / default-3 / … so
       parallel bare reviews never overwrite each other (each claims its own
       lane+round; a freshly bumped lane starts full, but a numbered lane that
       already has history continues incrementally — bump ≠ guaranteed full).
       Stable CROSS-ROUND parallel continuation still wants explicit, distinct
       --reviewer <lane> per reviewer.
     - completed round = round-NN/ dir containing review.md
     - bare lane: a round dir WITHOUT review.md marks the lane in-flight → the
       next bare call bumps past it (NOT resumed); named lane: still resumed (same NN)
     - snapshots the current committed non-plan diff into round-NN/diff-snapshot.patch
     - if a previous completed round exists in this lane, writes the unified diff
       of its snapshot vs the current snapshot to round-NN/diff-delta.patch
       (covers new commits, fix-ups AND rebases — the diff-vs-base is canonical)

  3. Mode: "full" if this lane has no completed rounds; "incremental" otherwise.

  Uncommitted working-tree changes are OUT of review scope (reported as NOTE).

stdout on success (KEY=VALUE, one per line; paths repo-relative):
  ROUND=2
  MODE=incremental
  POSTURE=standard | devils-advocate
  REVIEWER=<lane>
  BRANCH=<branch>
  BASE=<base ref> @ <short sha>
  HEAD=<short sha>
  STATE_DIR=temp/review-pr/<branch>/<lane>
  DIFF_SNAPSHOT=.../round-02/diff-snapshot.patch
  DIFF_DELTA=.../round-02/diff-delta.patch                     (or "none")
  PREV_REVIEWS=.../round-01/review.md                          (comma-joined, or "none")
  PLAN=<plan-file>                                             (or "none")
  LOCKED_GOAL_FILE=temp/review-pr/<branch>/.locked-goal        (or "none" when PLAN exists)
  TRIAGE_LEDGER=.../<ts1>/triage.md,.../<ts2>/triage.md        (PR mode only, mtime old→new, comma-joined, or "none")
  NOTE=...                                                     (zero or more)

Exit codes:
  0 — pass; 1 — prereq failed ("FAIL" + "- <reason>" lines); 2 — usage error

Usage:
    python3 skills/review-pr/scripts/review_round.py \
        [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan.md>]
"""

import argparse
import difflib
import re
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
TRIAGE_MODE_FIELD_RE = re.compile(
    r"(?im)^\s*(?:\*\*)?(?:review type|review mode|审查类型|模式)(?:\*\*)?\s*[:：]\s*(plan|pr|code|task)\b"
)
TRIAGE_MODE_TITLE_RE = re.compile(r"(?im)^#.*?[（(]\s*(plan|pr|code|task)\s*模式")
TRIAGE_MODE_ALIASES = {"plan": "plan", "pr": "pr", "code": "pr", "task": "task"}


def rel(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def git(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=REPO_ROOT, capture_output=True, text=True)


def read_locked_goal(goal_file: Path) -> str | None:
    """读取无计划 PR 的开发者锁定目标；不存在或只有空白时返回 None。"""
    if not goal_file.is_file():
        return None
    goal = goal_file.read_text().strip()
    return goal or None


def triage_mode(path: Path) -> str | None:
    """读取 author-response 台账显式声明的 review mode；未知格式 fail closed。"""
    try:
        content = path.read_text()
    except OSError:
        return None
    preamble = content.split("\n## ", 1)[0]
    match = TRIAGE_MODE_FIELD_RE.search(preamble) or TRIAGE_MODE_TITLE_RE.search(preamble)
    return TRIAGE_MODE_ALIASES.get(match.group(1).lower()) if match else None


def triage_ledgers(branch_slug: str) -> list[Path]:
    """本分支全部 PR-mode 裁决台账，排除 plan mode 与 task namespace，按 mtime 旧→新。

    triage.md 是每次 address-review 运行的产物、只含该次处理的 finding，**非累积**；只读最新
    一份会漏掉更早或别的 lane 裁决过的已决项，无法兑现「跨轮 / 跨 lane 抑制」。故返回全部台账，
    由 reviewer 全部读取、同一问题以较新台账的裁决为准。plan-mode 与 task-mode 台账不属于 PR
    CLOSED WORLD，不得用于抑制 code finding。只按 mtime 排序，不依赖目录名 / 轮号。无则返回 []。
    """
    address_root = REPO_ROOT / "temp" / "address-review-comments"
    triage_root = address_root / branch_slug
    task_namespace = address_root / "__task__"
    if not triage_root.is_dir():
        return []
    candidates = [
        t
        for t in triage_root.glob("*/triage.md")
        if t.is_file() and task_namespace not in t.parents and triage_mode(t) == "pr"
    ]
    return sorted(candidates, key=lambda p: p.stat().st_mtime)


def lanes_with_history(lane_root: Path) -> list[str]:
    """本分支下所有「已有已完成轮次（含 review.md）」的 lane 名，按名排序。

    用于裸调用（无 --reviewer）的归属安全检查：一旦存在真正的具名 lane，裸调用无法确定要续
    哪条，必须让用户显式 --reviewer（否则会静默落回 default、丢失跨轮归属）。
    """
    if not lane_root.exists():
        return []
    return [
        d.name
        for d in sorted(lane_root.iterdir())
        if d.is_dir() and any(r.is_dir() and (r / "review.md").is_file() for r in d.glob("round-*"))
    ]


# default 与 default-N 都是脚本为「单 reviewer / 首轮并发」自动认领的裸调用族（见 claim_bare_round），
# 不是用户显式 --reviewer 传入的具名 lane。
DEFAULT_LANE_RE = re.compile(r"default(-\d+)?")


def bare_call_ambiguous_lanes(prior_lanes: list[str]) -> list[str]:
    """裸调用无法自动归属时，返回导致歧义的 lane（非空即须显式 --reviewer）；可自动续则返回 []。

    可自动续（返回 []）的**唯一** solo 情形：无任何用户具名 lane，且 default 自动族至多一条已完成 lane
    （default 中断后落到 default-2 也算这一条，claim_bare_round 会在其上续写——见其文档）。
    歧义（返回全部已完成 lane 供报错）：
      - 存在用户显式 --reviewer 传入的具名 lane（多 reviewer）；或
      - **≥2 条** default 族已完成 lane（多个并发首轮各自完成 default / default-2，裸调用无法确定续哪条，
        续 default 会让原属 default-2 的 reviewer 读错 PREV_REVIEWS、破坏 lane 隔离）。
    """
    named = [ln for ln in prior_lanes if not DEFAULT_LANE_RE.fullmatch(ln)]
    default_family = [ln for ln in prior_lanes if DEFAULT_LANE_RE.fullmatch(ln)]
    if not named and len(default_family) <= 1:
        return []
    return sorted(named + default_family)


def claim_bare_round(lane_root: Path) -> tuple[str, Path, list[Path]]:
    """裸调用（无 --reviewer）下并发安全地认领一个轮次目录。

    优先续用稳定的 'default' lane（solo 反复裸调用即在此累积轮次）；若 'default' 当前
    存在一个「在飞行中」的轮次（round 目录已建但还没写 review.md——说明被另一个并发实例
    占用，或上次中断遗留），就原子地跳到下一条编号 lane（default-2 / default-3 …）。
    认领靠 `mkdir(exist_ok=False)` 的原子性：同一轮次目录只会被一个实例建成，竞争失败者
    自动顺延到下一条 lane，从而并发裸调用互不覆盖留痕。

    返回 (reviewer, 本轮 round 目录, 本 lane 已完成轮次列表)。

    代价（已知且可接受）：中断遗留的「在飞行中」轮次不再被原地复用，而是让活跃 lane 顺延
    一格（如 default 中断后 solo 续轮自动落到 default-2 并在其上继续）——不丢数据，仅 lane
    名漂移；这是换取并发不覆盖的取舍。
    """
    n = 1
    while True:
        lane = "default" if n == 1 else f"default-{n}"
        lane_dir = lane_root / lane
        rounds = sorted(d for d in lane_dir.glob("round-*") if d.is_dir()) if lane_dir.exists() else []
        completed = [d for d in rounds if (d / "review.md").is_file()]
        inflight = any(not (d / "review.md").is_file() for d in rounds)
        if inflight:
            n += 1
            continue
        target = lane_dir / f"round-{len(completed) + 1:02d}"
        try:
            target.mkdir(parents=True, exist_ok=False)
        except FileExistsError:
            n += 1
            continue
        return lane, target, completed


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--base", default="origin/master")
    parser.add_argument("--reviewer", default=None)
    parser.add_argument("--devils-advocate", action="store_true", dest="devils_advocate")
    parser.add_argument("--plan", default=None)
    try:
        args = parser.parse_args(argv[1:])
    except SystemExit:
        print(
            "usage: review_round.py [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan.md>]",
            file=sys.stderr,
        )
        return 2
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

    plan_path: Path | None = None
    if args.plan is not None:
        plan_path = Path(args.plan)
        if not plan_path.is_file():
            print(f"error: --plan {plan_path} not found", file=sys.stderr)
            return 2

    failures: list[str] = []
    branch = git("branch", "--show-current").stdout.strip()
    if not branch:
        failures.append("HEAD 处于 detached 状态——请在具名分支上运行 /review-pr")
    elif branch in ("main", "master"):
        failures.append("当前在 main/master 上，没有可审查的分支变更")
    if git("rev-parse", "--verify", "--quiet", args.base).returncode != 0:
        failures.append(f"base ref '{args.base}' 无法解析（是否需要先 git fetch origin？）")

    diff_text = ""
    if not failures:
        diff_proc = git(
            "diff",
            f"{args.base}...HEAD",
            "--",
            ".",
            ":(exclude)plans/**",
        )
        if diff_proc.returncode != 0:
            failures.append(f"git diff {args.base}...HEAD 失败：{diff_proc.stderr.strip()}")
        else:
            diff_text = diff_proc.stdout
            if not diff_text.strip():
                failures.append(
                    f"git diff {args.base}...HEAD has no committed non-plan changes after excluding plans/**"
                )

    if failures:
        print("FAIL")
        for f in failures:
            print(f"- {f}")
        return 1

    branch_slug = re.sub(r"[^A-Za-z0-9_-]", "-", branch)
    lane_root = REPO_ROOT / "temp" / "review-pr" / branch_slug
    locked_goal_file: Path | None = None
    if plan_path is None:
        locked_goal_file = lane_root / ".locked-goal"
        if read_locked_goal(locked_goal_file) is None:
            print("FAIL")
            print(
                "- 当前 PR 没有关联计划，也没有开发者锁定目标。请用一句话说明："
                "这个 PR 的单一 GOAL 是什么？收到回答后，reviewer 必须将原文写入 "
                f"{rel(locked_goal_file)} 再继续；不得从 diff、commit 或 PR 描述自行推断。"
            )
            return 1

    notes: list[str] = []
    if git("status", "--porcelain").stdout.strip():
        notes.append("工作树存在未提交变更——它们不在审查范围内（审查只覆盖已提交的 diff）")

    if default_lane:
        # 归属安全：裸调用只在「无历史」或「仅 default 有历史」时自动续轮。一旦存在具名 lane 或
        # 多条 lane 有历史（多 reviewer 场景），裸调用无法确定要续哪条——FAIL 要求显式 --reviewer，
        # 不再静默落回 default（那会丢失跨轮归属、让 reviewer 读串别人的历史）。
        prior_lanes = lanes_with_history(lane_root)
        ambiguous = bare_call_ambiguous_lanes(prior_lanes)
        if ambiguous:
            print("FAIL")
            print(
                "- 本分支已有多条 / 具名 reviewer lane，裸调用无法确定要续哪条；"
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
                "未指定 --reviewer，续用本 branch 的 'default' lane（裸调即自动续轮、先核销前轮）。"
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
    round_n = int(current.name.split("-")[1])

    (current / "diff-snapshot.patch").write_text(diff_text)

    delta_path: Path | None = None
    if completed:
        prev_snapshot = completed[-1] / "diff-snapshot.patch"
        if prev_snapshot.is_file():
            delta_lines = list(
                difflib.unified_diff(
                    prev_snapshot.read_text().splitlines(keepends=True),
                    diff_text.splitlines(keepends=True),
                    fromfile=f"{completed[-1].name}/diff-snapshot.patch",
                    tofile=f"{current.name}/diff-snapshot.patch",
                )
            )
            delta_path = current / "diff-delta.patch"
            delta_path.write_text("".join(delta_lines))
            if not delta_lines:
                notes.append("代码自上一轮审查以来无任何改动")
        else:
            notes.append(f"{completed[-1].name} 缺少 diff-snapshot.patch，本轮无增量 delta")

    base_sha = git("rev-parse", "--short", args.base).stdout.strip()
    head_sha = git("rev-parse", "--short", "HEAD").stdout.strip()

    print(f"ROUND={round_n}")
    # MODE 以「本 lane 是否存在已完成轮」为准，而非 round 编号：残留的 aborted 高编号
    # 轮次目录不应在没有任何 PREV_REVIEWS 的情况下把模式判成 incremental
    print(f"MODE={'incremental' if completed else 'full'}")
    print(f"POSTURE={'devils-advocate' if args.devils_advocate else 'standard'}")
    print(f"REVIEWER={reviewer}")
    print(f"BRANCH={branch}")
    print(f"BASE={args.base} @ {base_sha}")
    print(f"HEAD={head_sha}")
    print(f"STATE_DIR={rel(state_dir)}")
    print(f"DIFF_SNAPSHOT={rel(current / 'diff-snapshot.patch')}")
    print(f"DIFF_DELTA={rel(delta_path) if delta_path else 'none'}")
    prev_reviews = ",".join(rel(d / "review.md") for d in completed)
    print(f"PREV_REVIEWS={prev_reviews if prev_reviews else 'none'}")
    print(f"PLAN={rel(plan_path) if plan_path else 'none'}")
    print(f"LOCKED_GOAL_FILE={rel(locked_goal_file) if locked_goal_file else 'none'}")
    triage = triage_ledgers(branch_slug)
    print(f"TRIAGE_LEDGER={','.join(rel(t) for t in triage) if triage else 'none'}")
    for note in notes:
        print(f"NOTE={note}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
