#!/usr/bin/env python3
"""Gate check for /execute-plan: deterministic, agent-free pre-flight validation.

Uses the plan status as the sole review-readiness signal, then validates the
mechanical execution contract:

  0. Plan was generated from the canonical template (delegated to
     plan_template_check.check_template_match — catches users who try to
     execute against a custom / homegrown template).
  1. **状态** field exists and is one of {review-plan-complete,
     plan-execution-in-progress}.
  2. 参考资料 is non-empty (must contain at least one list item / link /
     file path, not just the prose intro and 重要 blockquote).
  3. 需要修改/添加的文件 contains a file contract; plain bullets are
     canonical and legacy checkbox bullets remain accepted without inspecting
     their checked state.
  4. 实施步骤 contains canonical task blocks with every required field.
  5. Task ids, dependencies, owner paths, and graph cycles pass the shared
     verify_task_graph validator.

Side effect: on PASS, this script also bumps **状态** in the plan file from
`review-plan-complete` → `plan-execution-in-progress`. This makes the gate
atomic — there's no window where a caller can forget to record that execution
started. Idempotent: if 状态 is already `plan-execution-in-progress` (resume
case), the file is left untouched.

Exit codes:
  0 — all checks pass; stdout is "PASS" (and a one-line note if the file was
      bumped to plan-execution-in-progress)
  1 — at least one check failed; stdout is "FAIL" followed by one
      "- <reason>" line per failure. Plan file is NOT modified.
  2 — usage error (bad args or plan file not found); message on stderr

Usage:
    python3 skills/execute-plan/scripts/plan_execution_gate.py <plan.md>
"""

import datetime
import re
import sys
from pathlib import Path


# Co-located sibling script — works when invoked as a file path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from plan_template_check import (  # noqa: E402
    check_template_match,
    detect_template,
)
from verify_task_graph import (  # noqa: E402
    CREATED_DATE_LABEL_RE,
    CREATED_DATE_RE,
    check_final_gate_contract,
    check_gate_execution_levels,
    check_plan_path_closure,
    check_task_fields,
    extract_section_body,
    parse_tasks,
    verify,
)


ALLOWED_STATUSES = frozenset({"review-plan-complete", "plan-execution-in-progress"})

STATUS_RE = re.compile(r"^\*\*状态\*\*：\s*(\S+)\s*$", re.MULTILINE)
EXAMPLE_BLOCK_RE = re.compile(
    r"<!--\s*示例[：:].*?开始.*?-->.*?<!--\s*示例[：:].*?结束\s*-->",
    re.DOTALL,
)
HTML_COMMENT_RE = re.compile(r"<!--.*?-->", re.DOTALL)


def strip_noise(body: str) -> str:
    """Drop <!-- 示例 --> blocks, HTML comments, blockquote lines."""
    body = EXAMPLE_BLOCK_RE.sub("", body)
    body = HTML_COMMENT_RE.sub("", body)
    body = "\n".join(line for line in body.split("\n") if not line.lstrip().startswith(">"))
    return body.strip()


def check_status(text: str) -> str | None:
    m = STATUS_RE.search(text)
    if not m:
        return "缺少 **状态** 字段（请使用 docs/templates/plan-template.md 最新模板）"
    status = m.group(1)
    if status not in ALLOWED_STATUSES:
        return (
            f"**状态** = '{status}'。/execute-plan 仅接受 "
            "'review-plan-complete'（计划审查通过后写入）或 "
            "'plan-execution-in-progress'（恢复执行）"
        )
    return None


def check_decisions_empty(decisions_body: str) -> str | None:
    """需要决策的事项 must have no remaining decision items."""
    cleaned = strip_noise(decisions_body)
    cleaned = re.sub(r"^\*\*当前计划完整程度\*\*：.*$", "", cleaned, flags=re.MULTILINE)
    item_re = re.compile(r"^\s*(?:\d+\.|[-*])\s+\*\*", re.MULTILINE)
    if item_re.search(cleaned):
        return "『需要决策的事项』仍有未解决项（应迁到『已归档的决策』后再执行）"
    return None


def check_section_has_content(text: str, heading: str) -> str | None:
    """Section must have actual content (list/link/code/heading), not just prose intro."""
    body = extract_section_body(text, heading)
    if not body:
        return f"缺少 section『{heading}』"
    cleaned = strip_noise(body)
    # 裸文件路径 / 裸 URL / 行内反引号代码也是有效内容（参考资料常见写法）
    has_content = bool(
        re.search(
            r"^(\s*[-*]\s|\s*\d+\.\s|\s*```|\s*###?\s|\s*\[.+\]\()",
            cleaned,
            re.MULTILINE,
        )
        or re.search(r"https?://\S+|`[^`\n]+`|^\s*\S+/\S+\s*$", cleaned, re.MULTILINE)
    )
    if not has_content:
        return f"『{heading}』看起来为空（没有列表项 / 子标题 / 代码块 / 链接）"
    return None


# 模板占位说明整行（作者应替换为真实目标）。整行匹配以便剔除——避免作者在占位行下方另起
# 一行写真实目标（占位行未删）时被子串匹配误判为「仍是占位」。
GOAL_PLACEHOLDER_RE = re.compile(r"^用\*\*一句话\*\*声明本计划.*$", re.MULTILINE)

# `## 目标` 引入模板的日期。自此日期（含）起**创建**的计划必须含该 section；更早的旧计划按
# 创建日期 grandfather 豁免（一次性引入必需 section 不应回溯作废历史计划——见 plan_template_check
# GRANDFATHERED_SECTIONS 的同源理由）。用创建日期而非全局豁免，才能对新模板计划强制、对旧计划放行。
GOAL_REQUIRED_SINCE = "2026-07-13"
GOAL_HEADING_RE = re.compile(r"^##\s+目标\s*$", re.MULTILINE)
# 创建日期字段**存在**（不论填没填真实日期）。用来区分两种「无真实日期」：
#   字段整行缺失 = 引入创建日期字段之前的旧模板计划 → grandfather 豁免；
#   字段在但为占位/非法（如 [YYYY-MM-DD]）= 用了当前模板却没填 → 当作新计划，仍要求目标。


def _is_valid_calendar_date(s: str) -> bool:
    """CREATED_DATE_RE 只保证 YYYY-MM-DD 的**格式**前缀，不保证是真实日历日期。
    这里再校验一次，堵住 2026-02-99 之类词法早于 cutoff、却非法的值绕过目标锁定。"""
    try:
        datetime.date.fromisoformat(s)
        return True
    except ValueError:
        return False


def check_goal_required(text: str) -> str | None:
    """自 GOAL_REQUIRED_SINCE 起创建的新计划必须含 `## 目标`；更早的旧计划 grandfather 豁免。

    有 `## 目标` → None（内容交给 check_goal_section）。无 `## 目标` 时按创建日期判定：
      - 有真实日期 ≥ cutoff → 缺目标即 FAIL（ISO 日期串字典序即时间序）
      - 有真实日期 < cutoff → 旧计划豁免
      - 无真实日期但**有创建日期字段**（占位 [YYYY-MM-DD] / 非法）→ 用了当前模板却没填，当作新计划 → FAIL
        （堵住占位日期绕过目标锁定：gate 不再信任一个未填的日期字段）
      - **创建日期字段整行缺失** → 引入该字段前的旧模板计划 → 豁免（保留大量无日期历史计划的兼容）
    """
    if GOAL_HEADING_RE.search(text):
        return None
    m = CREATED_DATE_RE.search(text)
    if m and _is_valid_calendar_date(m.group(1)):
        if m.group(1) >= GOAL_REQUIRED_SINCE:
            return (
                "缺少必需 section『## 目标』（自模板引入目标起创建的计划必须声明单一目标；更早的旧计划按创建日期豁免）"
            )
        return None
    # 到这里：无真实日历日期（占位 [YYYY-MM-DD] / 非法如 2026-02-99 / 缺失）。
    if CREATED_DATE_LABEL_RE.search(text):
        return (
            "缺少必需 section『## 目标』"
            "（创建日期为占位/未填——用了当前模板却没填真实日期，按新计划要求声明单一目标；"
            "补上真实创建日期后仍须有目标）"
        )
    return None


def check_goal_section(text: str) -> str | None:
    """『目标』必须声明真实的单一目标，而非模板占位文字。

    section 存在性由 check_goal_required 负责（`目标` 已 grandfather 出 required_sections，
    故 check_template_match 不报它；缺 `## 目标` 的新计划由 check_goal_required 按创建日期判）。
    本函数只在 `## 目标` 存在时校验其内容，避免同一根因双报。
    目标是散文（一句话目标陈述），list/link/code 那套 has_content 启发式不适用——
    strip_noise 去掉 **重要** blockquote 与注释后，剔除模板占位行，仍有实质内容才算已填写。
    """
    if not re.search(r"^##\s+目标\s*$", text, re.MULTILINE):
        return None
    cleaned = strip_noise(extract_section_body(text, "目标"))
    if not cleaned:
        return "『目标』看起来为空（只剩模板的 **重要** 说明，没有填写实际目标）"
    # 剔除模板占位说明行后仍无实质内容 → 作者没写真实目标（占位行 + 真实目标并存不误伤）
    if not GOAL_PLACEHOLDER_RE.sub("", cleaned).strip():
        return "『目标』仍是模板占位文字（用一句话声明本计划的单一目标后再执行）"
    return None


SIZE_RE = re.compile(r"^\*\*大小\*\*：\s*(.*?)\s*$", re.MULTILINE)
VALID_SIZES = frozenset({"XS", "S", "M", "L", "XL", "XXL", "XXXL"})
LOC_ESTIMATE_RE = re.compile(r"~?\s*\d+\s*LOC|约\s*\d+\s*行|\d+\s*行左右", re.IGNORECASE)


def check_size_token(text: str, require: bool) -> str | None:
    """大小 must be a single grade token — no parenthesized scope/file-count/LOC suffix.

    require=False (review time): empty / template placeholder is allowed (大小 is
    filled only after completeness ≥95%). require=True (execution gate): must be set.
    """
    m = SIZE_RE.search(text)
    if not m:
        return "缺少 **大小** 字段（请使用 docs/templates/plan-template.md 最新模板）" if require else None
    val = m.group(1).strip()
    if not val or val.startswith("["):
        return "**大小** 未填写（应为单一等级 token：XS/S/M/L/XL/XXL/XXXL）" if require else None
    if val.strip("`") not in VALID_SIZES:
        return (
            f"**大小** = '{val}' 不是单一等级 token（只允许 XS/S/M/L/XL/XXL/XXXL，"
            "禁止追加括号说明、scope、文件数量、行数或工作清单）"
        )
    return None


def check_loc_estimates(text: str) -> list[str]:
    """文件清单与实施步骤禁止 LOC / 行数估算（~20 LOC / 约 50 行 等伪精度）。"""
    failures: list[str] = []
    for heading in ("需要修改/添加的文件", "实施步骤"):
        cleaned = strip_noise(extract_section_body(text, heading))
        in_fence = False
        for line in cleaned.split("\n"):
            if line.lstrip().startswith("```"):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            for m in LOC_ESTIMATE_RE.finditer(line):
                failures.append(
                    f"『{heading}』含代码行数估算 '{m.group(0).strip()}'——删除 LOC/行数估算，只描述职责、边界与行为"
                )
    return failures


def gate(text: str) -> list[str]:
    failures: list[str] = []

    # 0. Plan must be generated from the canonical template.
    _, template_path = detect_template(text)
    if template_path.is_file():
        for tmpl_failure in check_template_match(text, template_path.read_text()):
            failures.append(tmpl_failure)
    else:
        failures.append(f"canonical 模板缺失：{template_path}（仓库结构异常）")

    if err := check_status(text):
        failures.append(err)

    if err := check_goal_required(text):
        failures.append(err)
    if err := check_goal_section(text):
        failures.append(err)
    if err := check_section_has_content(text, "参考资料"):
        failures.append(err)
    if err := check_section_has_content(text, "实施步骤"):
        failures.append(err)
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
    if err := check_size_token(text, require=True):
        failures.append(err)
    failures.extend(check_loc_estimates(text))

    if err := check_section_has_content(text, "需要修改/添加的文件"):
        failures.append(err)

    return failures


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(
            "usage: plan_execution_gate.py <plan.md>",
            file=sys.stderr,
        )
        return 2
    plan_path = Path(argv[1])
    if not plan_path.is_file():
        print(f"error: {plan_path} not found", file=sys.stderr)
        return 2

    text = plan_path.read_text()
    failures = gate(text)
    if failures:
        print("FAIL")
        for f in failures:
            print(f"- {f}")
        return 1

    # Atomic side effect: gate-pass also marks the plan as in-execution.
    status_match = STATUS_RE.search(text)
    current_status = status_match.group(1) if status_match else None
    if current_status == "review-plan-complete":
        new_text = STATUS_RE.sub("**状态**：plan-execution-in-progress", text, count=1)
        plan_path.write_text(new_text)
        print("PASS")
        print("- 状态：review-plan-complete → plan-execution-in-progress")
    else:
        # current_status == "plan-execution-in-progress" (resume case); no bump needed.
        print("PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
