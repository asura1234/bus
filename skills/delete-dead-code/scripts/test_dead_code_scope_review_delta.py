"""`review` reviews only this invocation's own assertion rewrites: rewrites already committed in the PR are not
this track's artifact to explain."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parent))

from dead_code_scope import rewritten_assertions  # noqa: E402


def _git(repository: Path, *arguments: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repository), *arguments],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def _write(repository: Path, relative: str, text: str) -> None:
    target = repository / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


def _commit(repository: Path, message: str) -> None:
    _git(repository, "add", "-A")
    _git(repository, "commit", "-q", "-m", message)


def test_committed_feature_assertion_change_is_not_listed_for_this_run(tmp_path: Path) -> None:
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    _git(tmp_path, "config", "user.email", "t@t")
    _git(tmp_path, "config", "user.name", "t")
    _write(tmp_path, "src/__tests__/a.test.ts", "expect(one).toBe(1)\n")
    _write(tmp_path, "src/prod.ts", "export const one = 1\n")
    _commit(tmp_path, "base")
    # The PR itself legitimately changed behavior and expectation and committed it; delete-dead-code then runs on this tree and changes no test.
    _write(tmp_path, "src/prod.ts", "export const one = 2\n")
    _write(tmp_path, "src/__tests__/a.test.ts", "expect(one).toBe(2)\n")
    _commit(tmp_path, "feature: one becomes 2")

    # SKILL.md A4 / B5 require every listed rewrite to be explained in this track's artifact, otherwise STOP;
    # this track rewrote no assertion, so none may be listed.
    assert rewritten_assertions(tmp_path) == []
