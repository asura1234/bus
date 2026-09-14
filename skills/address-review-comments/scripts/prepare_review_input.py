#!/usr/bin/env python3
"""校验并净化一个或多个 review.md，只保留作者可处置的两个 section。"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence


REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT))

_PREPARATION_PREFIX = "<!-- address-review-comments-preparation-v1 "
_PREPARATION_SUFFIX = " -->"


def preparation_identity_line(
    mode: str,
    input_kind: str,
    target: Mapping[str, str],
) -> str:
    """净化产物携带的身份行：mode / 输入种类 / 目标身份。

    它是给人和主 agent 读的 provenance，**不再是机器契约**——原先解析它的
    `claim_verification_support` 随逐 claim 核实协议一并删除，现在仓库里没有任何读取方。
    mode/target 的一致性由本脚本在合并多个输入时直接判定，不依赖回读这一行。格式仍由
    test_prepare_review_input.py 钉住，改动前先确认没有下游重新开始解析它。
    """

    encoded = json.dumps(
        {"mode": mode, "input_kind": input_kind, "target": dict(target)},
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    return f"{_PREPARATION_PREFIX}{encoded}{_PREPARATION_SUFFIX}"

from cli_extensions.review_artifact import (  # noqa: E402
    SUBSTANTIVE_HEADING,
    SYNC_HEADING,
    ReviewArtifact,
    ReviewArtifactError,
    ReviewMode,
    atomic_write,
    parse_review_artifact,
    validate_compatible,
)


PreparationError = ReviewArtifactError

# free-form review 没有 lane/round 身份，统一挂在这个前缀下，与结构化 lane 不会撞名。
FREE_FORM_LANE_PREFIX = "external:"
_VALID_MODES = ("plan", "pr", "task")
_REVISION_DECORATED_REF = re.compile(
    r"^(?P<ref>.+) @ (?P<revision>[0-9a-f]{7,64})$"
)


@dataclass(frozen=True)
class PreparedReviewInput:
    mode: ReviewMode
    input_kind: str
    target: Mapping[str, str]
    review_files: tuple[Path, ...]
    content: str


def _validate_provenance_path(path: Path) -> None:
    if any(char in str(path) for char in ("\r", "\n", "\0")):
        raise PreparationError("review 来源路径不得包含 CR、LF 或 NUL")


def _dedupe_paths(review_files: Sequence[Path]) -> tuple[Path, ...]:
    result: list[Path] = []
    seen: set[Path] = set()
    for raw_path in review_files:
        _validate_provenance_path(raw_path)
        path = raw_path.expanduser().resolve()
        _validate_provenance_path(path)
        if path in seen:
            continue
        if not path.is_file():
            raise PreparationError(f"review file 不存在: {path}")
        seen.add(path)
        result.append(path)
    return tuple(result)


def _review_ref(value: str, field: str) -> str:
    match = _REVISION_DECORATED_REF.fullmatch(value)
    if match is None:
        raise PreparationError(
            f"structured PR {field} 必须是 '<ref> @ <short-sha>': {value}"
        )
    normalized_ref = match.group("ref").strip()
    if not normalized_ref:
        raise PreparationError(
            f"structured PR {field} 必须是 '<ref> @ <short-sha>': {value}"
        )
    return normalized_ref


def _normalize_structured_target(
    mode: ReviewMode, target: Mapping[str, str]
) -> dict[str, str]:
    if mode != "pr":
        return dict(target)
    return {
        "branch": _review_ref(target["branch"], "branch"),
        "base": _review_ref(target["base"], "base"),
        "plan": "n/a" if target["plan"] == "无" else target["plan"],
        "locked_goal": target["locked_goal"],
    }


@dataclass(frozen=True)
class _SourceBlock:
    artifact: ReviewArtifact
    is_free_form: bool


def _source_tag(block: _SourceBlock) -> str:
    artifact = block.artifact
    finding_ids = ",".join(artifact.finding_ids) if artifact.finding_ids else "none"
    # free-form 没有轮次身份，如实写 n/a，不伪造 round 号——provenance 是裁决依据。
    # 判据是构造来源，不是 lane 字符串：lane 取自自由文本，按前缀猜会把某个
    # 恰好叫 external:… 的结构化 lane 的真实 round 抹成 n/a。
    round_label = "n/a" if block.is_free_form else str(artifact.round_number)
    return (
        f"> **来源**：mode={artifact.mode} | lane={artifact.lane} | "
        f"round={round_label} | file={artifact.path} | "
        f"finding-ids={finding_ids}"
    )


def _free_form_label(path: Path, label: str | None) -> str:
    resolved = label if label else path.stem
    if any(char in resolved for char in ("\r", "\n", "\0")):
        raise PreparationError("free-form 来源标签不得包含 CR、LF 或 NUL")
    return resolved


def _fence(text: str) -> str:
    """取一段比正文中最长连续反引号更长的围栏，保证正文无法逃逸。"""
    longest = 0
    current = 0
    for char in text:
        current = current + 1 if char == "`" else 0
        longest = max(longest, current)
    return "`" * max(3, longest + 1)


def _quarantine(text: str) -> str:
    """把 free-form 正文围进代码围栏。

    原文是外部输入，可能含 `## ` 标题——包括本脚本承诺的两个固定标题。
    直接拼接会伪造出额外 H2，破坏「只有两节」的输出契约，下游按固定标题
    切分时会把正文读到错误的节里。围栏内的内容不再被当作 Markdown 结构。
    """
    fence = _fence(text)
    return f"{fence}markdown\n{text}\n{fence}"


def _load_free_form(
    path: Path, mode: ReviewMode, label: str | None
) -> ReviewArtifact:
    """把任意 review 文本包成 artifact；不解析结构，原文整体作为 substantive。"""
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError as error:
        # UnicodeDecodeError 是 ValueError 而非 OSError，不转换就会绕过
        # main() 的统一错误处理，直接抛 traceback。
        raise PreparationError(
            f"free-form review 不是 UTF-8 文本: {path}"
        ) from error
    if not text.strip():
        raise PreparationError(f"free-form review 内容为空: {path}")
    return ReviewArtifact(
        path=path,
        mode=mode,
        round_number=0,
        lane=f"{FREE_FORM_LANE_PREFIX}{_free_form_label(path, label)}",
        target={},
        substantive=_quarantine(text.strip()),
        sync="",
        finding_ids=(),
        previous_summary=None,
        verdict="",
        recovery_reason=None,
        producer_task_id=None,
        text=text,
        headings=(),
    )


def _render(
    blocks: Sequence[_SourceBlock],
    mode: ReviewMode,
    input_kind: str,
    target: Mapping[str, str],
) -> str:
    identity_target = {} if input_kind == "free-form" else target
    lines = [
        preparation_identity_line(mode, input_kind, identity_target),
        "",
        SUBSTANTIVE_HEADING,
        "",
    ]
    for block in blocks:
        lines.extend([_source_tag(block), "", block.artifact.substantive, ""])
    lines.extend([SYNC_HEADING, ""])
    for block in blocks:
        lines.extend([_source_tag(block), "", block.artifact.sync, ""])
    return "\n".join(lines).rstrip() + "\n"


def prepare_review_input(
    review_files: Sequence[Path] | None = None,
    free_form_files: Sequence[Path] | None = None,
    mode: str | None = None,
    labels: Sequence[str] | None = None,
) -> PreparedReviewInput:
    """校验并净化 review 输入，生成两节 sanitized Markdown。

    结构化 review.md 仍走原来的 fail-closed 解析与兼容性校验；free-form
    review（GitHub 评审、粘贴的意见、任意 prompt）不解析结构，但必须显式
    声明 mode，因为裁决要靠它选 guide 与 landing 规则。
    """
    structured_paths = _dedupe_paths(review_files or [])
    free_form_paths = _dedupe_paths(free_form_files or [])
    if not structured_paths and not free_form_paths:
        raise PreparationError(
            "必须提供至少一个 --review-file 或 --free-form-file"
        )
    if mode is not None and mode not in _VALID_MODES:
        raise PreparationError(
            f"未知 review mode: {mode}（可选 {'/'.join(_VALID_MODES)}）"
        )

    # free-form 的 mode 必须由调用方显式声明，混用时也不例外：从结构化
    # artifact 推导会让「粘贴的 PR 评论 + plan review artifact」这种误配
    # 静默产出 mode=plan，裁决就会选错 guide 与 landing 规则。
    if free_form_paths and mode is None:
        raise PreparationError(
            "free-form review 没有可解析的身份，必须显式提供 --mode "
            "plan|pr|task"
        )

    try:
        structured = tuple(parse_review_artifact(path) for path in structured_paths)
    except ReviewArtifactError as error:
        raise PreparationError(
            f"{error}。migration 前的 review 记录必须作为自由文本输入："
            "使用 --free-form-file <path> --mode plan|pr"
        ) from error
    if structured:
        validate_compatible(structured)
        resolved_mode = structured[0].mode
        if mode is not None and mode != resolved_mode:
            raise PreparationError(
                f"--mode {mode} 与 review file 的 mode {resolved_mode} 冲突"
            )
        target = _normalize_structured_target(
            resolved_mode, structured[0].target
        )
    else:
        if mode is None:
            raise PreparationError(
                "free-form review 没有可解析的身份，必须显式提供 --mode "
                "plan|pr|task"
            )
        resolved_mode = mode
        target = {}

    label_list = list(labels or [])
    if label_list and len(label_list) != len(free_form_paths):
        raise PreparationError(
            f"--label 数量（{len(label_list)}）与 --free-form-file 数量"
            f"（{len(free_form_paths)}）不一致"
        )
    free_form = tuple(
        _load_free_form(
            path,
            resolved_mode,
            label_list[index] if label_list else None,
        )
        for index, path in enumerate(free_form_paths)
    )
    blocks = tuple(
        _SourceBlock(artifact, is_free_form=False) for artifact in structured
    ) + tuple(_SourceBlock(artifact, is_free_form=True) for artifact in free_form)
    input_kind = (
        "mixed"
        if structured and free_form
        else ("structured" if structured else "free-form")
    )

    # lane 是 provenance 的唯一标识，必须在结构化与 free-form 之间也唯一。
    seen_lanes: set[str] = set()
    for block in blocks:
        lane = block.artifact.lane
        if not block.is_free_form and lane.startswith(FREE_FORM_LANE_PREFIX):
            raise PreparationError(
                f"结构化 review 的 lane 不得使用保留前缀 "
                f"{FREE_FORM_LANE_PREFIX}: {lane}"
            )
        if lane in seen_lanes:
            raise PreparationError(f"review lane 重复，无法区分来源: {lane}")
        seen_lanes.add(lane)

    return PreparedReviewInput(
        mode=resolved_mode,
        input_kind=input_kind,
        target=target,
        review_files=structured_paths + free_form_paths,
        content=_render(blocks, resolved_mode, input_kind, target),
    )


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--review-file",
        action="append",
        type=Path,
        default=[],
        help="待净化的结构化 review.md，可重复传入",
    )
    parser.add_argument(
        "--free-form-file",
        action="append",
        type=Path,
        default=[],
        help="任意格式的 review 文本（GitHub 评审、粘贴意见、prompt），可重复传入",
    )
    parser.add_argument(
        "--mode",
        choices=_VALID_MODES,
        help="free-form 输入必填；与结构化 review file 同时使用时必须一致",
    )
    parser.add_argument(
        "--label",
        action="append",
        default=[],
        help="free-form 来源标签（如 copilot / codex / 人名），按顺序对应；缺省取文件名",
    )
    parser.add_argument("--output", type=Path, required=True, help="sanitized 输出路径")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        prepared = prepare_review_input(
            args.review_file,
            args.free_form_file,
            args.mode,
            args.label,
        )
        output = args.output.expanduser().resolve()
        if output in prepared.review_files:
            raise PreparationError("output 不得覆盖输入 review file")
        atomic_write(output, prepared.content)
    except (PreparationError, OSError) as error:
        sys.stderr.write(f"prepare-review-input error: {error}\n")
        return 2
    sys.stdout.write(f"{output}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
