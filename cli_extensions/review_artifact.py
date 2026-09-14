#!/usr/bin/env python3
"""校验 review.md，并确定性渲染开发者可读的聊天回复。"""

from __future__ import annotations

import argparse
import os
import sys
import tempfile
from pathlib import Path
from typing import Sequence


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import review_artifact_parser as _parser_module
import review_artifact_types as _types_module
from review_artifact_parser import (
    parse_review_artifact,
    render_chat_response,
)
from review_artifact_types import (
    ReviewArtifactError,
)


for _module in (_parser_module, _types_module):
    for _name in dir(_module):
        if not _name.startswith("__"):
            globals().setdefault(_name, getattr(_module, _name))
del _module, _name


def atomic_write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temp_name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    temp_path = Path(temp_name)
    try:
        # `newline="\n"` 不是讲究：文本模式默认 `newline=None`，写时把 `\n` 翻译成
        # `os.linesep`，Windows 上得到 CRLF。而这些产物按 canonical LF 消费，
        # 于是整条 review 链在 Windows 上必然失败——不是测试问题，是产品不可用。
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp_path, path)
    finally:
        temp_path.unlink(missing_ok=True)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    render = subparsers.add_parser(
        "render-response",
        help="校验完整 artifact，并生成唯一聊天回复",
    )
    render.add_argument("--review-file", type=Path, required=True)
    render.add_argument("--output", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    try:
        review_file = args.review_file.expanduser().resolve()
        output = args.output.expanduser().resolve()
        if output == review_file:
            raise ReviewArtifactError("output 不得覆盖输入 review file")
        artifact = parse_review_artifact(review_file)
        rendered = render_chat_response(artifact)
        atomic_write(output, rendered)
    except (ReviewArtifactError, OSError) as error:
        sys.stderr.write(f"review-artifact error: {error}\n")
        return 2
    sys.stdout.write(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
