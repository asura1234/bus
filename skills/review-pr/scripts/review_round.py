#!/usr/bin/env python3
"""Round prologue for /review-pr: deterministic prereq check + round bookkeeping.

No agent judgment, and NO repo mutation: the reviewer is read-only over the code;
the only writes are this script's own round artifacts under temp/review-pr/.

What it does:

  1. Prereq (unmet prerequisite → FAIL):
     - inside a git work tree, on a named branch that is not main/master
     - base ref (default origin/master) resolvable
     - branch mode: committed non-plan diff is non-empty; `plans/**` is excluded
     - --scope mode: a fixed file list / JSON chunk manifest replaces that diff;
       ordinary UTF-8 committed files are reviewed in full, even without branch changes
     - both branch-level locks, `.locked-goal` and `.locked-non-goals`, exist and are
       nonblank; with an associated plan both must equal the plan's Goal/Non-goals

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

  With --scope, lanes live under <branch>/scopes/<canonical manifest hash>/<reviewer>.
  Whole committed file snapshots replace the branch diff; later deltas compare the same lane's
  file contents and modes, including deletions / adopted tests. Missing prior snapshots fail closed.
  SCOPE_FILE, SCOPE_HASH, SCOPE_SNAPSHOT, and SCOPE_TEST_FILES are extra stdout fields.
  New declared dedicated test paths may be absent until adopted; probes may write only this allowlist.

  Uncommitted working-tree changes are OUT of both target kinds (reported as NOTE).

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
  LOCKED_GOAL_FILE=temp/review-pr/<branch>/.locked-goal
  LOCKED_NON_GOALS_FILE=temp/review-pr/<branch>/.locked-non-goals
  TRIAGE_LEDGER=.../<ts1>/triage.md,.../<ts2>/triage.md        (PR mode only, mtime old→new, comma-joined, or "none")
  NOTE=...                                                     (zero or more)

Exit codes:
  0 — pass; 1 — prereq failed ("FAIL" + "- <reason>" lines); 2 — usage error

Usage:
    python3 skills/review-pr/scripts/review_round.py \
        [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan.md>]
        [--scope <FILE_LIST|chunk.json>]
"""

import argparse
import difflib
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent

# Lane ownership, round claiming, and ledger discovery share one implementation with the other
# review skill in the repo-root cli_extensions/. This code
# once existed as byte-identical copies in both skills, and its drift fails silently: a wrong lane
# only makes the reviewer read someone else's PREV_REVIEWS, with no red light anywhere.
sys.path.insert(0, str(REPO_ROOT / "cli_extensions"))
from review_round_common import (  # noqa: E402
    bare_call_ambiguous_lanes,
    claim_bare_round,
    git,
    lanes_with_history,
    rel,
)
from review_round_common import (
    triage_ledgers as _triage_ledgers,
)
from review_scope import (  # noqa: E402
    ReviewScopeError,
    load_review_scope,
    load_scope_snapshot,
    scope_patch,
    snapshot_scope,
)

PR_SCRIPTS = REPO_ROOT / "skills" / "pr" / "scripts"
if str(PR_SCRIPTS) not in sys.path:
    sys.path.insert(0, str(PR_SCRIPTS))

# The lock writer owns plan section parsing; exact-match checks reuse that single parser.
from pr_goal_context import build_context  # noqa: E402


LOCKED_GOAL_NAME = ".locked-goal"
LOCKED_NON_GOALS_NAME = ".locked-non-goals"


class LockedContextError(ValueError):
    """Locked review Goal/Non-goals are missing, blank, or disagree with the plan."""


@dataclass(frozen=True)
class LockedReviewContext:
    goal: str
    non_goals: str
    goal_file: Path
    non_goals_file: Path


def triage_ledgers(branch_slug: str) -> list[Path]:
    """All **PR-mode** triage ledgers on this branch; other modes and task-mode are outside this CLOSED WORLD."""
    return _triage_ledgers(branch_slug, "pr")


def read_required_goal_and_non_goals(lane_root: Path) -> LockedReviewContext:
    """Read the branch-level Goal and Non-goals locks; fail closed when either is missing or blank."""
    goal_file = lane_root / LOCKED_GOAL_NAME
    non_goals_file = lane_root / LOCKED_NON_GOALS_NAME
    values: dict[Path, str] = {}
    missing: list[str] = []
    for path in (goal_file, non_goals_file):
        value = path.read_text(encoding="utf-8").strip() if path.is_file() else ""
        if value:
            values[path] = value
        else:
            missing.append(rel(path))
    if missing:
        raise LockedContextError("缺少或空白的锁定上下文：" + "、".join(missing))
    return LockedReviewContext(
        goal=values[goal_file],
        non_goals=values[non_goals_file],
        goal_file=goal_file,
        non_goals_file=non_goals_file,
    )


def require_exact_plan_match(context: LockedReviewContext, plan: Path) -> None:
    goal, non_goals = build_context([plan.read_text(encoding="utf-8")])
    mismatched = [
        label
        for label, locked, planned in (
            ("Goal", context.goal, goal.strip()),
            ("Non-goals", context.non_goals, non_goals.strip()),
        )
        if locked != planned
    ]
    if mismatched:
        raise LockedContextError(
            "锁定的 " + "、".join(mismatched) + f" 与计划 {rel(plan)} 不一致；不得自动覆盖"
        )


def load_locked_review_context(lane_root: Path, plan: Path | None) -> LockedReviewContext:
    context = read_required_goal_and_non_goals(lane_root)
    if plan is not None:
        require_exact_plan_match(context, plan)
    return context


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--base", default="origin/master")
    parser.add_argument("--reviewer", default=None)
    parser.add_argument("--devils-advocate", action="store_true", dest="devils_advocate")
    parser.add_argument("--plan", default=None)
    parser.add_argument("--scope", type=Path, default=None)
    try:
        args = parser.parse_args(argv[1:])
    except SystemExit:
        print(
            "usage: review_round.py [--base <ref>] [--reviewer <lane>] [--devils-advocate] [--plan <plan.md>] [--scope <file-list|manifest.json>]",
            file=sys.stderr,
        )
        return 2
    # Lane selection: a bare call (no --reviewer) continues the stable 'default' lane and claims a
    # round concurrency-safely -- see claim_bare_round below (when 'default' is occupied by a
    # concurrent run it auto-isolates to default-2/3..., never overwriting). Continuation does not
    # depend on the agent remembering any id, nor on --devils-advocate (lane is independent of
    # posture). Here we first validate a named lane: an empty result after sanitizing fails fast
    # (illegal characters become '-' and never empty the name, so this only fires when --reviewer
    # is an empty string), avoiding a silent fallback to default that would cross into bare calls.
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

    scope = None
    if args.scope is not None:
        try:
            scope = load_review_scope(args.scope, REPO_ROOT)
        except (ReviewScopeError, OSError, UnicodeError) as error:
            print(f"error: {error}", file=sys.stderr)
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
    if not failures and scope is None:
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
                failures.append(f"git diff {args.base}...HEAD 排除 plans 后没有可审查的已提交变更")

    if failures:
        print("FAIL")
        for f in failures:
            print(f"- {f}")
        return 1

    branch_slug = re.sub(r"[^A-Za-z0-9_-]", "-", branch)
    lane_root = REPO_ROOT / "temp" / "review-pr" / branch_slug
    try:
        locked = load_locked_review_context(lane_root, plan_path)
    except (LockedContextError, ValueError, OSError, UnicodeError) as error:
        print("FAIL")
        print(
            f"- {error}。两份 locks 只由 skills/pr/scripts/pr_goal_context.py 生成"
            "（--plan，或开发者提供的 --goal-file 与 --non-goal-file）；"
            "不得从 diff、commit 或 PR 描述自行推断。"
        )
        return 1

    # Branch locks remain owned by pr_goal_context; chunks isolate only round/ledger history.
    snapshot = None
    scope_head = None
    if scope is not None:
        lane_root = lane_root / "scopes" / scope.scope_hash
        scope_head = git("rev-parse", "HEAD").stdout.strip()
        try:
            snapshot = snapshot_scope(scope, REPO_ROOT, scope_head)
        except (ReviewScopeError, OSError, subprocess.CalledProcessError) as error:
            print("FAIL")
            print(f"- {error}")
            return 1

    notes: list[str] = []
    if git("status", "--porcelain").stdout.strip():
        notes.append(
            "工作树存在未提交变更——它们不在审查范围内（scope 审查只覆盖已提交的完整文件）"
            if scope is not None else
            "工作树存在未提交变更——它们不在审查范围内（审查只覆盖已提交的 diff）"
        )

    if default_lane:
        # Ownership safety: a bare call auto-continues only with "no history" or "only default has
        # history". Once a named lane or multiple lanes have history (multi-reviewer), a bare call
        # cannot tell which to continue -- FAIL and require an explicit --reviewer instead of
        # silently falling back to default (that loses cross-round ownership and lets a reviewer
        # read someone else's history).
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

    if scope is None:
        (current / "diff-snapshot.patch").write_text(diff_text)

    delta_path: Path | None = None
    if scope is not None:
        try:
            previous = load_scope_snapshot(completed[-1] / "scope-snapshot.json", scope) if completed else None
            if previous is None:
                if not any(entry is not None for entry in snapshot["files"].values()):
                    raise ReviewScopeError("scope 在 HEAD 中没有可审查的完整文件")
                missing = [p for p, entry in snapshot["files"].items() if entry is None and p not in scope.test_files]
                if missing:
                    raise ReviewScopeError("scope 首轮文件不在 HEAD 中: " + ",".join(missing))
            diff_text = scope_patch(None, snapshot)
            if previous is not None:
                delta_path = current / "diff-delta.patch"
                delta_text = scope_patch(previous, snapshot)
                delta_path.write_text(delta_text, encoding="utf-8", newline="\n")
                if not delta_text:
                    notes.append("chunk 代码自上一轮审查以来无任何改动")
            scope.write(lane_root / "scope.json")
            (current / "scope-snapshot.json").write_text(
                json.dumps(snapshot, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n"
            )
        except (ReviewScopeError, OSError) as error:
            print("FAIL")
            print(f"- {error}")
            return 1
    elif completed:
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

    if scope is not None:
        (current / "diff-snapshot.patch").write_text(diff_text, encoding="utf-8", newline="\n")

    base_sha = git("rev-parse", "--short", args.base).stdout.strip()
    head_sha = git("rev-parse", "--short", scope_head or "HEAD").stdout.strip()

    print(f"ROUND={round_n}")
    # MODE depends on whether this lane has a completed round, not on the round number: a leftover
    # aborted high-numbered round dir must not make the mode incremental without any PREV_REVIEWS
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
    print(f"LOCKED_GOAL_FILE={rel(locked.goal_file)}")
    print(f"LOCKED_NON_GOALS_FILE={rel(locked.non_goals_file)}")
    if scope is not None:
        print("TARGET_KIND=scope")
        print(f"SCOPE_HASH={scope.scope_hash}")
        print(f"SCOPE_FILE={rel(lane_root / 'scope.json')}")
        print(f"SCOPE_SNAPSHOT={rel(current / 'scope-snapshot.json')}")
        print(f"SCOPE_TEST_FILES={','.join(scope.test_files) if scope.test_files else 'none'}")
    triage = triage_ledgers(f"{branch_slug}/scopes/{scope.scope_hash}" if scope is not None else branch_slug)
    print(f"TRIAGE_LEDGER={','.join(rel(t) for t in triage) if triage else 'none'}")
    for note in notes:
        print(f"NOTE={note}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
