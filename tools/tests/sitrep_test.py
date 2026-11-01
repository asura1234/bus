"""The report renderer keeps the developer-facing format unambiguous."""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
RENDER = ROOT / "skills/sitrep/scripts/render.py"


@pytest.fixture(autouse=True)
def fake_lookups(tmp_path, monkeypatch):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    gh = bin_dir / "gh"
    gh.write_text("#!/usr/bin/env python3\nimport json, os\nprint(json.dumps({'state': os.environ.get('FAKE_PR_STATE', 'MERGED')}))\n")
    git = bin_dir / "git"
    git.write_text("#!/usr/bin/env python3\nimport os, sys\nargs = sys.argv[1:]\nif args[0] == 'symbolic-ref': print('origin/master')\nif args[0] == 'merge-base': sys.exit(int(os.environ.get('FAKE_COMMIT_LANDED', '0')))\n")
    gh.chmod(0o755)
    git.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")


def render(report, extra_env=None):
    env = dict(os.environ)
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        [sys.executable, str(RENDER)], input=json.dumps(report), text=True,
        capture_output=True, check=False, env=env,
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


@pytest.mark.parametrize("blocker", ["None", "None for you right now, but work is ongoing", "first\ncommentary", "first\u0085commentary", "first\u2028commentary", "", "  "])
def test_blocker_cannot_smuggle_none_or_extra_lines(blocker):
    result = render(report([blocker]))
    assert result.returncode != 0
    assert not result.stdout


def test_complete_report_and_invalid_status():
    data = report([])
    data["ongoing"] = [{"name": "Feature", "tldr": "Building it.", "context": "Fix round underway.", "status": "in-development", "pr": "[#12 Add it](https://example.com/pr/12)"}]
    data["finished"] = [{"name": "Fix", "tldr": "Merged.", "context": "Shipped today.", "pr": "merged [#11 Fix it](https://example.com/pr/11)"}]
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


def test_pr_title_with_bracketed_type_is_a_valid_link():
    data = report([])
    data["ongoing"] = [{"name": "Feature", "tldr": "In review.", "context": "Draft PR.", "status": "review", "pr": "[#141 [feat] add provider](https://github.com/example/repo/pull/141)"}]
    result = render(data)
    assert result.returncode == 0, result.stderr
    assert data["ongoing"][0]["pr"] in result.stdout


def test_pr_title_with_escaped_brackets_is_a_valid_link():
    data = report([])
    data["finished"] = [{"name": "Fix", "tldr": "Merged.", "context": "Complete.", "pr": r"merged [#12 fix \[unmatched](https://example.com/pr/12)"}]
    result = render(data)
    assert result.returncode == 0, result.stderr
    assert data["finished"][0]["pr"] in result.stdout


def test_empty_finished_is_exactly_none():
    result = render(report([]))
    assert result.returncode == 0, result.stderr
    finished = result.stdout.split("## Deferred")[0].split("## Finished (since the last sitrep)\n", 1)[1]
    assert finished == "None\n\n"


@pytest.mark.parametrize("item", [
    {"name": "Feature", "tldr": "Waiting to merge.", "context": "Approved.", "pr": "[#12 Add it](https://example.com/pr/12)"},
    {"name": "Feature", "tldr": "Gate-green.", "context": "Ready but unmerged.", "pr": "[#12 Add it](https://example.com/pr/12)"},
    {"name": "Feature", "tldr": "Approved.", "context": "Reviewers signed off.", "pr": "[#12 Add it](https://example.com/pr/12)"},
    {"name": "None", "tldr": "none merged, but work is ready.", "context": "Still open.", "pr": "[#12 Add it](https://example.com/pr/12)"},
    {"name": "Feature", "tldr": "Shipped.", "context": "No pull request.", "pr": "not opened yet (branch: feat/sitrep-skill)"},
    {"name": "Fix", "tldr": "Merged.", "context": "Shipped today.", "pr": "[#11 Fix it](https://example.com/pr/11)"},
])
def test_finished_rejects_items_without_merge_evidence(item):
    data = report([])
    data["finished"] = [item]
    result = render(data)
    assert result.returncode != 0
    assert not result.stdout


def test_finished_accepts_only_a_merged_pr_or_landed_commit():
    merged = report([])
    merged["finished"] = [{"name": "Fix", "tldr": "Merged.", "context": "Shipped today.", "pr": "merged [#11 Fix it](https://example.com/pr/11)"}]
    merged_result = render(merged)
    assert merged_result.returncode == 0, merged_result.stderr
    assert merged_result.stdout.split("## Deferred")[0].endswith(
        "## Finished (since the last sitrep)\n- Fix\n  - TLDR: Merged.\n  - Context: Shipped today.\n  - PR/MR: merged [#11 Fix it](https://example.com/pr/11)\n\n"
    )
    landed = report([])
    sha = "7cd03ec7a69d4881223643e68064f69edc385369"
    landed["finished"] = [{"name": "Fix", "tldr": "Landed.", "context": "On the default branch.", "pr": f"landed {sha}"}]
    landed_result = render(landed)
    assert landed_result.returncode == 0, landed_result.stderr
    assert f"  - PR/MR: landed {sha}\n" in landed_result.stdout


def test_finished_rejects_pr_not_merged_in_gh():
    data = report([])
    data["finished"] = [{"name": "Fix", "tldr": "Merged.", "context": "Shipped today.", "pr": "merged [#11 Fix it](https://example.com/pr/11)"}]
    result = render(data, {"FAKE_PR_STATE": "OPEN"})
    assert result.returncode != 0
    assert not result.stdout


def test_finished_rejects_commit_not_on_default_branch():
    data = report([])
    data["finished"] = [{"name": "Fix", "tldr": "Landed.", "context": "On the default branch.", "pr": "landed 6ce810d4641d81c7144752fcadbc0498a4671a16"}]
    result = render(data, {"FAKE_COMMIT_LANDED": "1"})
    assert result.returncode != 0
    assert not result.stdout


def test_finished_uses_real_state_instead_of_status_prose():
    data = report([])
    data["finished"] = [{"name": "Fix", "tldr": "Ready.", "context": "The work is ready.", "pr": "merged [#11 Fix it](https://example.com/pr/11)"}]
    result = render(data)
    assert result.returncode == 0, result.stderr
    assert "TLDR: Ready." in result.stdout


def test_unmerged_work_stays_under_ongoing():
    data = report([])
    data["ongoing"] = [{"name": "Feature", "tldr": "Waiting to merge.", "context": "Approved and gate-green.", "status": "review", "pr": "[#12 Add it](https://example.com/pr/12)"}]
    ongoing = render(data)
    assert ongoing.returncode == 0, ongoing.stderr
    assert "Waiting to merge." in ongoing.stdout.split("## Finished")[0]
    data["ongoing"] = []
    data["finished"] = [{"name": "Feature", "tldr": "Waiting to merge.", "context": "Approved and gate-green.", "pr": "merged [#12 Add it](https://example.com/pr/12)"}]
    finished = render(data, {"FAKE_PR_STATE": "OPEN"})
    assert finished.returncode != 0
    assert not finished.stdout
