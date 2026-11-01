"""Sitrep state comes from fixture Git history and the main-worktree marker."""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "skills/sitrep/scripts/marker.py"


def git(repo, *args):
    subprocess.run(["git", *args], cwd=repo, text=True, capture_output=True, check=True)


def commit(repo, message):
    path = repo / "work.txt"
    with path.open("a") as output:
        output.write(message + "\n")
    git(repo, "add", "work.txt")
    git(repo, "commit", "-m", message)
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()


@pytest.fixture
def fixture_repo(tmp_path):
    origin = tmp_path / "origin.git"
    main = tmp_path / "main"
    subprocess.run(["git", "init", "--bare", str(origin)], capture_output=True, check=True)
    subprocess.run(["git", "init", "--initial-branch=master", str(main)], capture_output=True, check=True)
    git(main, "config", "user.name", "Sitrep Test")
    git(main, "config", "user.email", "sitrep@example.com")
    base = commit(main, "Base")
    git(main, "remote", "add", "origin", str(origin))
    git(main, "push", "-u", "origin", "master")
    git(main, "remote", "set-head", "origin", "--auto")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    gh = bin_dir / "gh"
    gh.write_text("#!/usr/bin/env python3\nimport os\nprint(os.environ.get('FAKE_OPEN_PRS', '[]'))\n")
    gh.chmod(0o755)
    env = {"PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}"}
    return {"origin": origin, "main": main, "base": base, "env": env}


def call(repo, fixture, action="read", start=None, extra_env=None, state=None):
    args = [sys.executable, str(SCRIPT), action, "--repo", str(repo)]
    if start:
        args.extend(["--start", start])
    if state:
        args.extend(["--state", str(state)])
    env = dict(fixture["env"])
    if extra_env:
        env.update(extra_env)
    return subprocess.run(args, text=True, capture_output=True, check=False, env=env)


def save_marker(fixture):
    path = fixture["main"] / "temp/sitrep/last-sitrep-commit"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(fixture["base"] + "\n")
    return path


def test_git_log_and_open_prs_compute_state_then_marker_advances(fixture_repo):
    marker = save_marker(fixture_repo)
    main = fixture_repo["main"]
    landed = commit(main, "Fix finished work (#12)")
    git(main, "push", "origin", "master")
    pulls = [{"number": 13, "title": "Open [feature]", "url": "https://example.com/pr/13"}]
    result = call(main, fixture_repo, extra_env={"FAKE_OPEN_PRS": json.dumps(pulls)})
    assert result.returncode == 0, result.stderr
    state = json.loads(result.stdout)
    assert state == {
        "default_ref": "origin/master",
        "head": landed,
        "finished": [{"name": "Fix finished work (#12)", "pr": f"landed {landed}"}],
        "open_prs": [r"[#13 Open \[feature\]](https://example.com/pr/13)"],
    }
    state_path = main.parent / "state.json"
    state_path.write_text(result.stdout)
    saved = call(main, fixture_repo, action="write", state=state_path)
    assert saved.returncode == 0, saved.stderr
    assert marker.read_text() == landed + "\n"
    empty = call(main, fixture_repo)
    assert empty.returncode == 0, empty.stderr
    assert json.loads(empty.stdout)["finished"] == []


def test_marker_does_not_advance_past_the_rendered_head(fixture_repo):
    marker = save_marker(fixture_repo)
    main = fixture_repo["main"]
    rendered_head = commit(main, "Rendered head")
    git(main, "push", "origin", "master")
    result = call(main, fixture_repo)
    assert result.returncode == 0, result.stderr
    commit(main, "Landed after rendering")
    git(main, "push", "origin", "master")
    state_path = main.parent / "state.json"
    state_path.write_text(result.stdout)
    saved = call(main, fixture_repo, action="write", state=state_path)
    assert saved.returncode == 0, saved.stderr
    assert marker.read_text() == rendered_head + "\n"


def test_missing_marker_requires_an_explicit_start(fixture_repo):
    main = fixture_repo["main"]
    missing = call(main, fixture_repo)
    assert missing.returncode != 0 and not missing.stdout
    assert "no saved sitrep commit" in missing.stderr
    landed = commit(main, "Explicit start")
    git(main, "push", "origin", "master")
    started = call(main, fixture_repo, start=fixture_repo["base"])
    assert started.returncode == 0, started.stderr
    assert json.loads(started.stdout)["finished"] == [{"name": "Explicit start", "pr": f"landed {landed}"}]
    malformed = main / "temp/sitrep/last-sitrep-commit"
    malformed.parent.mkdir(parents=True, exist_ok=True)
    malformed.write_text("not-a-sha\n")
    invalid = call(main, fixture_repo)
    assert invalid.returncode != 0 and not invalid.stdout


def test_linked_worktree_uses_main_worktree_marker(fixture_repo):
    marker = save_marker(fixture_repo)
    main = fixture_repo["main"]
    landed = commit(main, "Main worktree marker")
    git(main, "push", "origin", "master")
    linked = main.parent / "linked"
    git(main, "worktree", "add", "-b", "feature", str(linked))
    result = call(linked, fixture_repo)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["finished"] == [{"name": "Main worktree marker", "pr": f"landed {landed}"}]
    state_path = main.parent / "state.json"
    state_path.write_text(result.stdout)
    saved = call(linked, fixture_repo, action="write", state=state_path)
    assert saved.returncode == 0, saved.stderr
    assert json.loads(saved.stdout)["saved"] == str(marker)
    assert marker.read_text() == landed + "\n"
    assert not (linked / "temp/sitrep/last-sitrep-commit").exists()


def test_nonancestor_marker_fails_closed(fixture_repo):
    path = save_marker(fixture_repo)
    path.write_text("0" * 40 + "\n")
    result = call(fixture_repo["main"], fixture_repo)
    assert result.returncode != 0 and not result.stdout
    assert "not an ancestor" in result.stderr
