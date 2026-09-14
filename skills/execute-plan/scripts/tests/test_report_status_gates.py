"""task agent report 的契约闸门只对完成态强制。"""

import hashlib
import subprocess
import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = SCRIPTS_DIR.parents[2]
SCRIPT = SCRIPTS_DIR / "task_agent_report.py"

SNAPSHOT = """# Task snapshot

### 任务 1：示例任务

- **验收闸门**：[TASK_LOCAL] `./run test cli` exit 0
"""


def _write_report(tmp_path: Path, status: str) -> tuple[Path, Path]:
    generation = tmp_path / "generation-01"
    generation.mkdir()
    snapshot = tmp_path / "task-1-snapshot.md"
    snapshot.write_text(SNAPSHOT)
    owner_gap = "- PATH: `a/b.ts`\n- REASON: 完成任务必须触及" if status == "OWNER_GAP" else "无。"
    blocker = "缺少上游契约。" if status in {"NEEDS_CONTEXT", "BLOCKED"} else "无。"
    report = generation / "report.md"
    report.write_text(
        "# Task Agent Report\n\n"
        "- TASK_ID: 1\n"
        "- TASK_NAME: 示例任务\n"
        "- GENERATION: 1\n"
        f"- STATUS: {status}\n"
        f"- SNAPSHOT: {snapshot}\n"
        f"- SNAPSHOT_SHA256: {hashlib.sha256(snapshot.read_bytes()).hexdigest()}\n"
        f"- TOUCHED_CONTENT_SHA256: {hashlib.sha256(b'').hexdigest()}\n"
        "\n## 实现摘要\n\n无。\n"
        "\n## Touched files\n\n无。\n"
        "\n## Gate evidence\n\n无。\n"
        f"\n## Owner gap\n\n{owner_gap}\n"
        "\n## Concerns\n\n无。\n"
        f"\n## Context or blocker\n\n{blocker}\n"
    )
    return report, snapshot


def _validate(report: Path, snapshot: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        [
            sys.executable,
            str(SCRIPT),
            "validate",
            "--repo",
            str(REPO_ROOT),
            "--report",
            str(report),
            "--task-id",
            "1",
            "--task-name",
            "示例任务",
            "--snapshot",
            str(snapshot),
        ],
        capture_output=True,
        text=True,
    )


@pytest.mark.parametrize("status", ["OWNER_GAP", "NEEDS_CONTEXT", "BLOCKED"])
def test_non_completed_status_does_not_require_contract_gates(tmp_path, status):
    """未完成态按定义在跑闸门之前就停下；仍强制契约命令会让 repair 唯一出口不可达。"""

    report, snapshot = _write_report(tmp_path, status)
    result = _validate(report, snapshot)
    assert result.returncode == 0, result.stderr
    assert f"STATUS={status}" in result.stdout


def test_completed_status_still_requires_contract_gates(tmp_path):
    report, snapshot = _write_report(tmp_path, "DONE")
    result = _validate(report, snapshot)
    assert result.returncode != 0
