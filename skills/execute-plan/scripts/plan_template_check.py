#!/usr/bin/env python3
"""Verify a plan was generated from the canonical template.

A plan passes this check iff, for the canonical template
(docs/templates/plan-template.md):

  1. Every required `## ` section appears in the plan. "Required" =
     every `## ` heading in the template that does NOT contain
     "（如适用）". Optional sections are ignored.
  2. Every immutable-block MARKER PAIR FROM A POPULATED template block
     appears in the plan — i.e. for each `<!-- <name>：开始 - 此部分不
     可修改 -->` / `<!-- <name>：结束 -->` pair in the template that has
     non-empty body, both markers exist in the plan with non-empty body
     between them.

     Why not byte-match the block bodies? Plans are immutable snapshots
     tied to a base commit. Template content evolves; a plan generated
     last quarter legitimately carries last-quarter's rule wording. The
     markers themselves are the proof of canonical origin — a custom
     template wouldn't ship the `此部分不可修改` markers verbatim, and an
     adversary who bothers to fake the markers AND populate them would
     also bother to write a real plan.

Exit codes:
  0 — plan matches the canonical template; stdout is "PASS"
  1 — at least one mismatch; stdout is "FAIL" then "- <reason>" lines
  2 — usage error (bad args or files not found); stderr message

Usage:
    python3 skills/execute-plan/scripts/plan_template_check.py <plan.md>

Also importable: `from plan_template_check import check_template_match`.
"""

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
TEMPLATE_PATH = REPO_ROOT / "docs" / "templates" / "plan-template.md"

H2_RE = re.compile(r"^##\s+(.+?)\s*$", re.MULTILINE)
IMMUTABLE_BLOCK_RE = re.compile(
    r"<!--\s*(.+?)：开始\s*-\s*此部分不可修改\s*-->(.*?)<!--\s*\1：结束\s*-->",
    re.DOTALL,
)
# Used against plans (no '- 此部分不可修改' qualifier required, since the body may
# have evolved but the marker should still bear the canonical name).
PLAN_BLOCK_OPEN_RE_TEMPLATE = r"<!--\s*{name}：开始[^>]*-->"
PLAN_BLOCK_CLOSE_RE_TEMPLATE = r"<!--\s*{name}：结束\s*-->"


def detect_template(plan_text: str) -> tuple[str, Path]:
    # Single canonical template in this repo; signature kept for gate compatibility.
    return ("regular", TEMPLATE_PATH)


# Sections the gate does NOT require of a plan. Two reasons a section lands here:
#   - added to the template AFTER plans were already in flight — a newly-required H2
#     must not invalidate every historical plan (plans are immutable snapshots tied to
#     a base commit; see module docstring). New plans still get it via create-plan; its
#     CONTENT (when present) is validated by the OTHER gates — check_goal_section /
#     check_goal_required in plan_execution_gate — NOT by this structure-matcher.
#   - intentionally OPTIONAL for all plans (e.g. 非目标: 缺省即无非目标；开发者不声明就没有，
#     脚本不校验其存在性也不校验内容——它只是给 review/author 一个显式的「刻意不做」围栏）。
GRANDFATHERED_SECTIONS = frozenset({"目标", "非目标"})


def required_sections(template_text: str) -> list[str]:
    return [
        m.group(1)
        for m in H2_RE.finditer(template_text)
        if "（如适用）" not in m.group(1) and m.group(1) not in GRANDFATHERED_SECTIONS
    ]


def immutable_block_names(template_text: str) -> list[str]:
    # Only blocks the template author intentionally populates carry enforcement
    # meaning. Skip blocks whose template body is empty/whitespace.
    return [m.group(1) for m in IMMUTABLE_BLOCK_RE.finditer(template_text) if m.group(2).strip()]


def check_template_match(plan_text: str, template_text: str) -> list[str]:
    # Return list of mismatch reasons; empty list means PASS.
    failures: list[str] = []

    for section in required_sections(template_text):
        section_re = re.compile(rf"^##\s+{re.escape(section)}\s*$", re.MULTILINE)
        if not section_re.search(plan_text):
            failures.append(f"缺少必需 section『## {section}』")

    for name in immutable_block_names(template_text):
        open_re = re.compile(PLAN_BLOCK_OPEN_RE_TEMPLATE.format(name=re.escape(name)))
        close_re = re.compile(PLAN_BLOCK_CLOSE_RE_TEMPLATE.format(name=re.escape(name)))
        open_match = open_re.search(plan_text)
        close_match = close_re.search(plan_text)
        if not open_match or not close_match or close_match.start() <= open_match.end():
            failures.append(
                f"缺少『{name}』immutable-block 标记对"
                f"（应有 `<!-- {name}：开始 ... -->` 和 `<!-- {name}：结束 -->`）；"
                "看起来计划没有基于 canonical 模板生成"
            )
            continue
        body = plan_text[open_match.end() : close_match.start()].strip()
        if not body:
            failures.append(f"『{name}』immutable-block 内容为空")

    return failures


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: plan_template_check.py <plan.md>", file=sys.stderr)
        return 2

    plan_path = Path(argv[1])
    if not plan_path.is_file():
        print(f"error: {plan_path} not found", file=sys.stderr)
        return 2

    plan_text = plan_path.read_text()
    _, template_path = detect_template(plan_text)
    if not template_path.is_file():
        print(f"error: template {template_path} not found", file=sys.stderr)
        return 2

    template_text = template_path.read_text()
    failures = check_template_match(plan_text, template_text)

    if failures:
        print("FAIL（与 canonical 模板比对）")
        for f in failures:
            print(f"- {f}")
        return 1

    print("PASS（匹配 canonical 模板）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
