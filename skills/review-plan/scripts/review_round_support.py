#!/usr/bin/env python3
"""Round prologue for /review-plan: deterministic prereq check + round bookkeeping.

No agent judgment, and — unlike plan_execution_gate.py — NO plan mutation:
状态 writes are out of scope for /review-plan (the developer asks for them
explicitly).

What it does:

  1. Prereq (前提不满足 → FAIL):
     - plan was generated from the canonical template (plan_template_check)
     - **状态** ∈ {create-plan-complete, review-plan-in-progress}
       (in-progress 计划未完成 / plan-execution-* 已进执行阶段 / abandoned 一律拒绝)
     - 需要决策的事项 has NO open items — decisions are resolved by the developer
       with the plan author during create-plan (per plan-template 计划生成规则,
       the plan body isn't even generated until they are); open items mean the
       plan isn't review-ready, not that a special review mode should run

  Plan file is optional: one branch = one plan = one purpose. Omitting it
  resumes the plan recorded for this branch (temp/review-plan/<branch_slug>/.last-plan);
  with no record, it auto-detects the single plans/*.md changed on the branch
  (0 or >1 candidates → FAIL asking to specify). This lets the developer run bare
  `/review-plan` repeatedly until convergence without re-typing the path.

  2. Round bookkeeping under temp/review-plan/<branch_slug>/<plan_basename>/<reviewer>/:
     - keyed by branch so bare invocations resume the same lane; per-plan-stem
       sub-dir keeps the rare two-plans-on-one-branch case from mixing history
     - --reviewer omitted → the stable 'default' lane, so repeated bare
       `/review-plan` continues the SAME lane and accumulates rounds (this is
       what makes 前轮问题核销 non-empty). Concurrent bare invocations are
       handled safely by claim_bare_round: if 'default' is in-flight (another
       live instance), the call atomically bumps to default-2 / default-3 / …
       so parallel bare reviews never overwrite each other (each claims its own
       lane+round; a freshly bumped lane starts full, but a numbered lane that
       already has history continues incrementally — bump ≠ guaranteed full).
       Stable CROSS-ROUND parallel continuation (1 writer + claude/codex/codex)
       still wants explicit, distinct --reviewer <lane> per reviewer.
     - completed round = round-NN/ dir containing review.md
     - bare lane: a round dir WITHOUT review.md marks the lane in-flight → the
       next bare call bumps past it (NOT resumed); named lane: still resumed (same NN)
     - snapshots the current plan into round-NN/plan-snapshot.md
     - if a previous completed round exists in this lane, writes the unified
       diff of its snapshot vs the current plan to round-NN/plan-diff.patch

  3. Mode: "full" if this lane has no completed rounds; "incremental" otherwise.
     (决策评估不属于审查轮次：已归档的决策是开发者的既定选择，开发者需要意见时
     手动单独征询 agent，不在 /review-plan 内。)

  The plan 状态 is the durable workflow signal; the temp history is only an
  accelerator. If 状态 is review-plan-in-progress but no round history exists
  in this lane (temp wiped / fresh worktree / new reviewer), this is reported
  as a NOTE and the lane's round counter starts at 1 — not an error.

stdout on success (KEY=VALUE, one per line; paths repo-relative):
  ROUND=2
  MODE=incremental
  POSTURE=standard | devils-advocate
  REVIEWER=<lane>
  BRANCH=<branch>
  PLAN=<resolved plan-file>
  STATE_DIR=temp/review-plan/<branch_slug>/<basename>/<lane>
  SNAPSHOT=.../round-02/plan-snapshot.md
  DIFF=.../round-02/plan-diff.patch                            (or "none")
  PREV_REVIEWS=.../round-01/review.md                          (comma-joined, or "none")
  TRIAGE_LEDGER=.../<ts1>/triage.md,.../<ts2>/triage.md        (plan mode only, mtime old→new, comma-joined, or "none")
  NOTE=...                                                     (zero or more)

Exit codes:
  0 — pass; 1 — prereq failed ("FAIL" + "- <reason>" lines); 2 — usage error

Usage:
    python3 skills/review-plan/scripts/review_round.py [<plan.md>] \
        [--reviewer <lane>] [--devils-advocate]
    python3 skills/review-plan/scripts/review_round.py <plan.md> --check   # 只跑前提门禁，read-only
"""

import re
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent

# Shared deterministic checks live in the execute-plan skill — import, don't duplicate.
sys.path.insert(0, str(REPO_ROOT / "skills" / "execute-plan" / "scripts"))
from plan_execution_gate import (  # noqa: E402
    STATUS_RE,
    check_decisions_empty,
    check_goal_required,
    check_goal_section,
    check_loc_estimates,
    check_size_token,
)
from plan_template_check import (  # noqa: E402
    check_template_match,
    detect_template,
)
from verify_task_graph import (  # noqa: E402
    check_final_gate_contract,
    check_gate_execution_levels,
    check_plan_path_closure,
    check_task_fields,
    extract_section_body,
    parse_tasks,
    verify,
)


ALLOWED_STATUSES = frozenset({"create-plan-complete", "review-plan-in-progress"})
LEGACY_STATUS_RE = re.compile(r"(?m)^-\s*Status:\s*\S+")
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


def current_branch() -> str:
    # 一个分支一个计划：lane 历史按分支归档，bare `/review-plan` 才能在同分支延续轮次。
    # detached / 非 git 环境回退到 _detached_ 哨兵，仍能正常按 plan stem 留痕。
    branch = git("branch", "--show-current").stdout.strip()
    return branch or "_detached_"


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
    """本分支全部 plan-mode 裁决台账，排除 PR mode 与 task namespace，按 mtime 旧→新。

    triage.md 是每次 address-review 运行的产物、只含该次处理的 finding，**非累积**；只读最新
    一份会漏掉更早或别的 lane 裁决过的已决项，无法兑现「跨轮 / 跨 lane 抑制」。故返回全部台账，
    由 reviewer 全部读取、同一问题以较新台账的裁决为准。PR-mode 与 task-mode 台账不属于计划
    CLOSED WORLD，不得用于抑制 plan finding。只按 mtime 排序，不依赖目录名 / 轮号。无则返回 []。
    """
    address_root = REPO_ROOT / "temp" / "address-review-comments"
    triage_root = address_root / branch_slug
    task_namespace = address_root / "__task__"
    if not triage_root.is_dir():
        return []
    candidates = [
        t
        for t in triage_root.glob("*/triage.md")
        if t.is_file() and task_namespace not in t.parents and triage_mode(t) == "plan"
    ]
    return sorted(candidates, key=lambda p: p.stat().st_mtime)


def detect_branch_plan_docs() -> list[Path]:
    """Return the unique plans/*.md changed against origin/master, including dirty files."""
    names: set[str] = set()
    diff = git("diff", "--name-only", "origin/master...HEAD")
    if diff.returncode == 0:
        names.update(diff.stdout.split())
    for line in git("status", "--porcelain").stdout.splitlines():
        # porcelain 行形如 'XY <path>'，截断状态码取路径；rename/copy 形如 'old -> new'，取新路径
        path = line[3:].strip()
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if path:
            names.add(path)
    plans = {REPO_ROOT / n for n in names if n.startswith("plans/") and n.endswith(".md")}
    return sorted(p for p in plans if p.is_file())


def prereq_failures(text: str, *, check_only: bool = False) -> list[str]:
    # check_only=True 是 create-plan 的**落盘前预检**：计划此刻仍是 create-plan-in-progress
    #   （尚未 flip 到 complete），只想确认「结构是否已达可审门槛」。此模式额外接受
    #   create-plan-in-progress，从而 create-plan 可以在 **still-in-progress** 状态下预检、
    #   只有 PASS 才 flip 到 create-plan-complete——避免「先置 complete、门禁再 FAIL」留下
    #   错误的持久状态。真正的 /review-plan 门禁（check_only=False）仍只接受 complete / review-in-progress。
    if STATUS_RE.search(text) is None and LEGACY_STATUS_RE.search(text):
        return [
            "检测到 migration 前的 legacy plan 格式。历史内容不会被自动改写；"
            "请显式调用 create-plan，以 docs/templates/plan-template.md 新建 canonical successor 后再 review-plan"
        ]

    allowed = ALLOWED_STATUSES | ({"create-plan-in-progress"} if check_only else frozenset())
    failures: list[str] = []

    _, template_path = detect_template(text)
    if template_path.is_file():
        failures.extend(check_template_match(text, template_path.read_text()))
    else:
        failures.append(f"canonical 模板缺失：{template_path}（仓库结构异常）")

    m = STATUS_RE.search(text)
    if not m:
        failures.append("缺少 **状态** 字段（请使用 docs/templates/plan-template.md 最新模板）")
    elif m.group(1) not in allowed:
        failures.append(
            f"**状态** = '{m.group(1)}'。/review-plan 仅接受 "
            "'create-plan-complete'（计划落盘待审）或 "
            "'review-plan-in-progress'（继续审查）"
        )

    # 决策评估不属于审查轮次：模板规则下计划正文（大小/实施步骤等）在决策全部解决前
    # 不会生成，存在未解决决策项即意味着计划尚不具备审查条件（前提不满足），
    # 而不是需要一个特殊的"决策审查模式"
    decisions_body = extract_section_body(text, "需要决策的事项")
    if decisions_body and check_decisions_empty(decisions_body) is not None:
        failures.append(
            "『需要决策的事项』仍有未解决项——决策由开发者在 create-plan 阶段解决"
            "（计划正文在决策解决前不会生成），解决后再提交审查"
        )

    # 目标是 create-plan 阶段就应填好的散文陈述（同 当前状态分析 / 参考资料），
    # review 时必须已声明单一目标：新计划必须含 `## 目标`（旧计划按创建日期豁免），且不得占位或空
    if err := check_goal_required(text):
        failures.append(err)
    if err := check_goal_section(text):
        failures.append(err)

    # 格式类机械检查：任务字段、身份、依赖、owner 与环都由单一 verifier 负责。
    # 大小 在 review 阶段允许为空（完整度 ≥95% 后才填写）
    task_failures = check_task_fields(text)
    failures.extend(task_failures)
    if not task_failures:
        try:
            tasks = parse_tasks(text)
        except ValueError as error:
            failures.extend(str(error).splitlines())
        else:
            failures.extend(verify(tasks))
            failures.extend(check_plan_path_closure(text, tasks))
            failures.extend(check_gate_execution_levels(text, tasks))
            failures.extend(check_final_gate_contract(text))
    if err := check_size_token(text, require=False):
        failures.append(err)
    failures.extend(check_loc_estimates(text))
    return failures


def lanes_with_history(lane_root: Path) -> list[str]:
    """本计划下所有「已有已完成轮次（含 review.md）」的 lane 名，按名排序。

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
