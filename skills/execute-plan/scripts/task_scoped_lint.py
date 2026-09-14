#!/usr/bin/env python3
"""按 generation delta 生成 task runtime scoped lint 命令。"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from task_agent_evidence import (  # noqa: E402
    EvidenceError,
    existing_touched_files,
    scoped_lint_command,
)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--file", action="append", default=[])
    args = parser.parse_args(argv)
    try:
        existing = existing_touched_files(
            args.repo.resolve(),
            tuple(args.file),
        )
        print(scoped_lint_command(existing) if existing else "NOT_APPLICABLE")
        return 0
    except (OSError, EvidenceError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
