#!/usr/bin/env python3
"""有界等待任一 task completion 文件，提供 runtime-independent 完成信号。"""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path


MAX_TIMEOUT_SECONDS = 60.0
POLL_INTERVAL_SECONDS = 0.25


class WaitError(ValueError):
    """等待参数不符合 completion 路径契约。"""


def _completion_paths(values: list[str]) -> tuple[Path, ...]:
    if not values:
        raise WaitError("至少需要一个 --completion")
    paths: list[Path] = []
    identities: set[Path] = set()
    for value in values:
        path = Path(value)
        if path.name != "completion.txt":
            raise WaitError(f"completion 文件名必须是 completion.txt：{value}")
        identity = path.resolve()
        if identity in identities:
            raise WaitError(f"completion 路径不得重复：{value}")
        identities.add(identity)
        paths.append(path)
    return tuple(paths)


def wait_for_completions(paths: tuple[Path, ...], timeout_seconds: float) -> tuple[Path, ...]:
    """在有界窗口内返回已原子发布的 completion 路径。"""

    if timeout_seconds < 0 or timeout_seconds > MAX_TIMEOUT_SECONDS:
        raise WaitError(f"--timeout-seconds 必须在 0 到 {MAX_TIMEOUT_SECONDS:g} 之间")
    deadline = time.monotonic() + timeout_seconds
    while True:
        ready = tuple(path.resolve() for path in paths if path.is_file())
        if ready:
            return ready
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return ()
        time.sleep(min(POLL_INTERVAL_SECONDS, remaining))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--completion", action="append", default=[])
    parser.add_argument("--timeout-seconds", type=float, default=60.0)
    args = parser.parse_args(argv)
    try:
        paths = _completion_paths(args.completion)
        ready = wait_for_completions(paths, args.timeout_seconds)
    except WaitError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if not ready:
        print("TIMEOUT")
        return 0
    for path in ready:
        print(f"READY={path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
