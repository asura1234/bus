"""The renderer reconciles the report with mechanically computed state."""
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
RENDER = ROOT / "skills/sitrep/scripts/render.py"
SHA_A = "7cd03ec7a69d4881223643e68064f69edc385369"
SHA_B = "6ce810d4641d81c7144752fcadbc0498a4671a16"


def state(finished=None, open_prs=None):
    return {"default_ref": "origin/master", "head": SHA_B, "finished": finished or [], "open_prs": open_prs or []}


def render(report, computed=None):
    if computed is None:
        computed = state(open_prs=[item["pr"] for item in report.get("ongoing", []) if item["pr"].startswith("[#")])
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "state.json"
        path.write_text(json.dumps(computed))
        return subprocess.run(
            [sys.executable, str(RENDER), "--state", str(path)], input=json.dumps(report), text=True,
            capture_output=True, check=False,
        )


def report(blockers):
    return {"blockers": blockers, "ongoing": [], "finished": [], "deferred": []}


def finished(name, sha, tldr="Merged."):
    return {"name": name, "tldr": tldr, "context": "Shipped today.", "pr": f"landed {sha}"}


def computed_finished(name, sha):
    return {"name": name, "pr": f"landed {sha}"}


def test_empty_blockers_is_exactly_none():
    result = render(report([]))
    assert result.returncode == 0, result.stderr
    assert result.stdout.split("## Ongoing")[0] == "## Blockers\nNone\n\n"


def test_blockers_are_only_bullets():
    result = render(report(["Waiting for dependency", "Developer decision needed"]))
    assert result.returncode == 0, result.stderr
    assert result.stdout.split("## Ongoing")[0] == "## Blockers\n- Waiting for dependency\n- Developer decision needed\n\n"


@pytest.mark.parametrize("blocker", ["None", "None for you right now, but work is ongoing", "first\ncommentary", "first\u0085commentary", "first\u2028commentary", "", "  "])
def test_blocker_cannot_smuggle_none_or_extra_lines(blocker):
    result = render(report([blocker]))
    assert result.returncode != 0
    assert not result.stdout


def test_complete_report_and_invalid_status():
    data = report([])
    data["ongoing"] = [{"name": "Feature", "tldr": "Building it.", "context": "Fix round underway.", "status": "in-development", "pr": "[#12 Add it](https://example.com/pr/12)"}]
    data["finished"] = [finished("Fix it (#11)", SHA_A)]
    data["deferred"] = [{"name": "Later", "tldr": "On hold.", "context": "Waiting for scheduling."}]
    computed = state(finished=[computed_finished("Fix it (#11)", SHA_A)], open_prs=[data["ongoing"][0]["pr"]])
    result = render(data, computed)
    assert result.returncode == 0, result.stderr
    assert "  - Status: in-development\n  - PR/MR: [#12 Add it](https://example.com/pr/12)" in result.stdout
    assert "## Finished (since the last sitrep)\n- Fix it (#11)\n  - TLDR: Merged." in result.stdout
    assert "## Deferred\n- Later\n  - TLDR: On hold." in result.stdout
    data["ongoing"][0]["status"] = "green"
    invalid = render(data, computed)
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
    data["ongoing"] = [{"name": "Feature", "tldr": "In review.", "context": "Draft PR.", "status": "review", "pr": r"[#12 fix \[unmatched](https://example.com/pr/12)"}]
    result = render(data)
    assert result.returncode == 0, result.stderr
    assert data["ongoing"][0]["pr"] in result.stdout


def test_empty_finished_is_exactly_none():
    result = render(report([]))
    assert result.returncode == 0, result.stderr
    finished_section = result.stdout.split("## Deferred")[0].split("## Finished (since the last sitrep)\n", 1)[1]
    assert finished_section == "None\n\n"


@pytest.mark.parametrize("item", [
    {"name": "Feature", "tldr": "Waiting to merge.", "context": "Approved.", "pr": "[#12 Add it](https://example.com/pr/12)"},
    {"name": "Feature", "tldr": "Shipped.", "context": "No pull request.", "pr": "not opened yet (branch: feat/sitrep-skill)"},
    {"name": "Fix", "tldr": "Merged.", "context": "Shipped today.", "pr": "merged [#11 Fix it](https://example.com/pr/11)"},
])
def test_finished_rejects_items_without_computed_landed_commits(item):
    data = report([])
    data["finished"] = [item]
    result = render(data)
    assert result.returncode != 0
    assert not result.stdout


def test_finished_accepts_computed_landed_commit():
    data = report([])
    data["finished"] = [finished("Fix it (#11)", SHA_A)]
    result = render(data, state(finished=[computed_finished("Fix it (#11)", SHA_A)]))
    assert result.returncode == 0, result.stderr
    assert f"  - PR/MR: landed {SHA_A}\n" in result.stdout


def test_finished_rejects_commit_not_on_default_branch():
    data = report([])
    data["finished"] = [finished("Not landed", SHA_B)]
    result = render(data)
    assert result.returncode != 0
    assert not result.stdout


@pytest.mark.parametrize("mismatch", ["drop", "add", "reorder", "rename"])
def test_finished_fails_closed_on_computed_log_mismatch(mismatch):
    expected = [computed_finished("First", SHA_A), computed_finished("Second", SHA_B)]
    items = [finished("First", SHA_A), finished("Second", SHA_B)]
    if mismatch == "drop":
        items = items[:1]
    elif mismatch == "add":
        items.append(finished("Extra", "0" * 40))
    elif mismatch == "reorder":
        items.reverse()
    else:
        items[0]["name"] = "Rewritten"
    data = report([])
    data["finished"] = items
    result = render(data, state(finished=expected))
    assert result.returncode != 0
    assert not result.stdout


def test_ongoing_fails_closed_on_open_pr_mismatch():
    first = "[#1 First](https://example.com/pr/1)"
    second = "[#2 Second](https://example.com/pr/2)"
    data = report([])
    data["ongoing"] = [{"name": "Second", "tldr": "Open.", "context": "Still open.", "status": "review", "pr": second}, {"name": "First", "tldr": "Open.", "context": "Still open.", "status": "review", "pr": first}]
    result = render(data, state(open_prs=[first, second]))
    assert result.returncode != 0
    assert not result.stdout


def test_finished_uses_computed_state_instead_of_status_prose():
    data = report([])
    data["finished"] = [finished("Fix", SHA_A, tldr="Ready.")]
    result = render(data, state(finished=[computed_finished("Fix", SHA_A)]))
    assert result.returncode == 0, result.stderr
    assert "TLDR: Ready." in result.stdout


def test_unmerged_work_stays_under_ongoing():
    data = report([])
    data["ongoing"] = [{"name": "Feature", "tldr": "Waiting to merge.", "context": "Approved and gate-green.", "status": "review", "pr": "[#12 Add it](https://example.com/pr/12)"}]
    ongoing = render(data)
    assert ongoing.returncode == 0, ongoing.stderr
    assert "Waiting to merge." in ongoing.stdout.split("## Finished")[0]
    data["ongoing"] = []
    data["finished"] = [{"name": "Feature", "tldr": "Waiting to merge.", "context": "Approved and gate-green.", "pr": "[#12 Add it](https://example.com/pr/12)"}]
    finished_result = render(data)
    assert finished_result.returncode != 0
    assert not finished_result.stdout
