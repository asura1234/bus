#!/usr/bin/env python3
"""Round prologue for /review-plan: deterministic prereq check + round bookkeeping.

No agent judgment and NO plan mutation:
状态 writes are out of scope for /review-plan (the developer asks for them
explicitly).

What it does:

  1. Prereq (前提不满足 → FAIL):
     - plan was generated from the canonical template (plan_template_check)
     - normal review rounds accept **状态** ∈
       {create-plan-complete, review-plan-in-progress}
     - read-only ``--check`` additionally accepts create-plan-in-progress for
       the plan author's preflight and review-plan-complete for publication checks
       (plan-execution-* has entered execution / abandoned are always rejected)
     - 需要决策的事项 has NO open items — decisions are resolved by the developer
       with the plan author while writing the plan (per the plan-template 计划生成规则,
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
     (Decision evaluation is not a review round: archived decisions are the developer's
     settled choices; when the developer wants an opinion they ask an agent separately,
     outside /review-plan.)

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
    python3 skills/review-plan/scripts/review_round.py <plan.md> --check   # structure/status gate only, read-only; can verify a plan whose review is complete
"""

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent

# Lane ownership, round claiming, and ledger discovery share one implementation with the other review
# skill (skills/AGENTS.md: shared Python lives in the repo-root cli_extensions/). This part used to exist
# as a byte-identical copy in each of two skills, and its drift fails silently: a wrong lane only makes a
# reviewer read someone else's PREV_REVIEWS, with no red light at all.
sys.path.insert(0, str(REPO_ROOT / "cli_extensions"))
from review_round_common import (  # noqa: E402
    git,
)
from review_round_common import (  # noqa: E402
    triage_ledgers as _triage_ledgers,
)


def triage_ledgers(branch_slug: str) -> list[Path]:
    """All **plan-mode** triage ledgers on this branch; other modes are outside this CLOSED WORLD."""
    return _triage_ledgers(branch_slug, "plan")


# Shared deterministic plan checks live beside this module.
sys.path.insert(0, str(Path(__file__).resolve().parent))
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


def current_branch() -> str:
    # One branch, one plan: lane history is filed per branch so bare `/review-plan` can continue rounds on it.
    # Detached / non-git environments fall back to the _detached_ sentinel and still record per plan stem.
    branch = git("branch", "--show-current").stdout.strip()
    return branch or "_detached_"


def detect_branch_plan_docs() -> list[Path]:
    """Return the unique plans/*.md changed against origin/master, including dirty files."""
    names: set[str] = set()
    diff = git("diff", "--name-only", "origin/master...HEAD")
    if diff.returncode == 0:
        names.update(diff.stdout.split())
    for line in git("status", "--porcelain").stdout.splitlines():
        # A porcelain line looks like 'XY <path>'; strip the status code to get the path; rename/copy looks like 'old -> new', take the new path
        path = line[3:].strip()
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if path:
            names.add(path)
    plans = {REPO_ROOT / n for n in names if n.startswith("plans/") and n.endswith(".md")}
    return sorted(p for p in plans if p.is_file())


def prereq_failures(text: str, *, check_only: bool = False) -> list[str]:
    # check_only=True is a read-only structure/status validation that creates no review round:
    #   - the plan author preflights while still-in-progress and flips to complete only on PASS;
    #   - commit/publish re-validates the finalized plan at review-plan-complete.
    # The real /review-plan gate (check_only=False) still accepts only create-plan-complete /
    # review-plan-in-progress and cannot reopen a plan whose review is complete.
    if STATUS_RE.search(text) is None and LEGACY_STATUS_RE.search(text):
        return [
            "检测到 migration 前的 legacy plan 格式。历史内容不会被自动改写；"
            "请以 docs/templates/plan-template.md 新建 canonical successor 后再 review-plan"
        ]

    allowed = ALLOWED_STATUSES | (
        {"create-plan-in-progress", "review-plan-complete"}
        if check_only
        else frozenset()
    )
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
        if check_only:
            failures.append(
                f"**状态** = '{m.group(1)}'。/review-plan --check 仅接受 "
                "'create-plan-in-progress'（落盘前预检）、"
                "'create-plan-complete'（计划落盘待审）、"
                "'review-plan-in-progress'（继续审查）或 "
                "'review-plan-complete'（发布前复验）"
            )
        else:
            failures.append(
                f"**状态** = '{m.group(1)}'。/review-plan 仅接受 "
                "'create-plan-complete'（计划落盘待审）或 "
                "'review-plan-in-progress'（继续审查）"
            )

    # Decision evaluation is not a review round: under the template rules the plan body (size / implementation
    # steps, etc.) is not generated until every decision is resolved, so an unresolved decision item means the
    # plan is not yet reviewable (prerequisite unmet), not that a special "decision review mode" is needed
    decisions_body = extract_section_body(text, "需要决策的事项")
    if decisions_body and check_decisions_empty(decisions_body) is not None:
        failures.append(
            "『需要决策的事项』仍有未解决项——决策由开发者在计划撰写阶段解决"
            "（计划正文在决策解决前不会生成），解决后再提交审查"
        )

    # The goal is a prose statement that should be filled in while writing the plan (like 当前状态分析 / 参考资料);
    # at review time a single goal must be declared: new plans must contain `## 目标` (old plans are exempt by
    # creation date), and it must not be a placeholder or empty
    if err := check_goal_required(text):
        failures.append(err)
    if err := check_goal_section(text):
        failures.append(err)

    # Mechanical format checks: task fields, identity, dependencies, owners, and cycles all belong to one verifier.
    # 大小 may be empty during review (it is filled in only once completeness is >=95%)
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
