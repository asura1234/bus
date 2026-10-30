"""The report renderer keeps the developer-facing format unambiguous."""
import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
RENDER = ROOT / "skills/sitrep/scripts/render.py"


def render(report):
    return subprocess.run(
        [sys.executable, str(RENDER)], input=json.dumps(report), text=True,
        capture_output=True, check=False,
    )


def report(blockers):
    return {"blockers": blockers, "ongoing": [], "finished": [], "deferred": []}


def test_empty_blockers_is_exactly_none():
    result = render(report([]))
    assert result.returncode == 0, result.stderr
    assert result.stdout.split("## Ongoing")[0] == "## Blockers\nNone\n\n"


def test_blockers_are_only_bullets():
    result = render(report(["Waiting for dependency", "Developer decision needed"]))
    assert result.returncode == 0, result.stderr
    assert result.stdout.split("## Ongoing")[0] == (
        "## Blockers\n- Waiting for dependency\n- Developer decision needed\n\n"
    )


@pytest.mark.parametrize("blocker", ["None", "None for you right now, but work is ongoing", "first\ncommentary", "first\u0085commentary", "", "  "])
def test_blocker_cannot_smuggle_none_or_extra_lines(blocker):
    result = render(report([blocker]))
    assert result.returncode != 0
    assert not result.stdout


def test_complete_report_and_invalid_status():
    data = report([])
    data["ongoing"] = [{"name": "Feature", "tldr": "Building it.", "context": "Fix round underway.", "status": "in-development", "pr": "[#12 Add it](https://example.com/pr/12)"}]
    data["finished"] = [{"name": "Fix", "tldr": "Merged.", "context": "Shipped today.", "pr": "[#11 Fix it](https://example.com/pr/11)"}]
    data["deferred"] = [{"name": "Later", "tldr": "On hold.", "context": "Waiting for scheduling."}]
    result = render(data)
    assert result.returncode == 0, result.stderr
    assert "  - Status: in-development\n  - PR/MR: [#12 Add it](https://example.com/pr/12)" in result.stdout
    assert "## Finished (since the last sitrep)\n- Fix\n  - TLDR: Merged." in result.stdout
    assert "## Deferred\n- Later\n  - TLDR: On hold." in result.stdout
    data["ongoing"][0]["status"] = "green"
    invalid = render(data)
    assert invalid.returncode != 0
    assert not invalid.stdout
