#!/usr/bin/env python3
"""Validate and sanitize one or more review.md files, keeping only the two sections the author can act on."""

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
    """The identity line carried by the sanitized output: mode / input kind / target identity.

    It is provenance for humans and the main agent to read, **no longer a machine contract**: the
    `claim_verification_support` that used to parse it was deleted together with the per-claim
    verification protocol, and nothing in the repo reads it now. mode/target consistency is decided
    directly by this script when merging multiple inputs, without reading this line back. The format is
    still pinned by prepare_review_input_test.py; before changing it, first confirm no downstream has
    started parsing it again.
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

# A free-form review has no lane/round identity; all of them hang under this prefix, so they never collide with structured lanes.
FREE_FORM_LANE_PREFIX = "external:"
_VALID_MODES = ("plan", "pr")
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
    # free-form has no round identity, so write n/a truthfully and do not forge a round number:
    # provenance is the basis for adjudication.
    # The criterion is how the block was constructed, not the lane string: the lane comes from free text,
    # and guessing by prefix would erase the real round of a structured lane that happens to be named
    # external:… into n/a.
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
    """Pick a fence longer than the longest run of consecutive backticks in the body, so the body cannot escape."""
    longest = 0
    current = 0
    for char in text:
        current = current + 1 if char == "`" else 0
        longest = max(longest, current)
    return "`" * max(3, longest + 1)


def _quarantine(text: str) -> str:
    """Wrap the free-form body in a code fence.

    The original text is external input and may contain `## ` headings, including the two fixed
    headings this script promises. Concatenating it directly would forge extra H2s and break the
    "exactly two sections" output contract; downstream splitting by the fixed headings would then read
    the body into the wrong section. Content inside the fence is no longer treated as Markdown structure.
    """
    fence = _fence(text)
    return f"{fence}markdown\n{text}\n{fence}"


def _load_free_form(
    path: Path, mode: ReviewMode, label: str | None
) -> ReviewArtifact:
    """Wrap arbitrary review text into an artifact; the structure is not parsed, and the whole original text becomes the substantive section."""
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError as error:
        # UnicodeDecodeError is a ValueError, not an OSError; without conversion it would bypass
        # main()'s unified error handling and surface as a raw traceback.
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
    """Validate and sanitize review input, producing two-section sanitized Markdown.

    Structured review.md still goes through the original fail-closed parsing and compatibility
    validation; a free-form review (GitHub review, pasted comments, any prompt) is not parsed for
    structure, but its mode must be declared explicitly, because adjudication relies on it to choose
    the guide and landing rules.
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

    # The free-form mode must be declared explicitly by the caller, mixed input included: deriving it
    # from the structured artifact would let a mismatch like "pasted PR comments + plan review artifact"
    # silently produce mode=plan, and adjudication would then pick the wrong guide and landing rules.
    if free_form_paths and mode is None:
        raise PreparationError(
            "free-form review 没有可解析的身份，必须显式提供 --mode "
            "plan|pr"
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
                "plan|pr"
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

    # The lane is the unique identifier of provenance, so it must also be unique across structured and free-form.
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
        help="Structured review.md to sanitize; may be passed multiple times",
    )
    parser.add_argument(
        "--free-form-file",
        action="append",
        type=Path,
        default=[],
        help="Review text in any format (GitHub review, pasted comments, prompt); may be passed multiple times",
    )
    parser.add_argument(
        "--mode",
        choices=_VALID_MODES,
        help="Required for free-form input; must match when used together with structured review files",
    )
    parser.add_argument(
        "--label",
        action="append",
        default=[],
        help="Source label for free-form input (e.g. copilot / codex / a person's name), matched in order; defaults to the file name",
    )
    parser.add_argument("--output", type=Path, required=True, help="Sanitized output path")
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
