"""Host parity of the pr and merge-pr executable flows.

The skills are the implementation under test: an agent runs their commands.
These cases execute that control flow against a Codeup host and a stale
origin HEAD, and fail while the flow still takes the GitHub path.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest


REPO = Path(__file__).resolve().parents[4]
MERGE_SKILL = REPO / "skills/merge-pr/SKILL.md"
PR_SKILL = REPO / "skills/pr/SKILL.md"
REBASE_BASE = REPO / "skills/rebase-origin-main/scripts/rebase_base.py"


def _section(text: str, start: str, end: str) -> str:
    begin = text.index(start)
    return text[begin:text.index(end, begin)]


def _codeup_runs(section: str) -> list[str]:
    """Lines that execute when host is codeup.

    An `IF host == github` applies only to the following indented lines.
    Unindented lines run for every host. An `IF host == codeup` block runs.
    """

    ran: list[str] = []
    mode = "all"
    for line in section.splitlines():
        stripped = line.strip()
        if stripped.startswith("IF host == github"):
            mode = "github"
            continue
        if stripped.startswith("IF host == codeup"):
            mode = "codeup"
            continue
        if stripped.startswith("=========="):
            mode = "all"
        indent = len(line) - len(line.lstrip(" "))
        if mode == "github" and indent == 0 and stripped:
            mode = "all"
        if mode == "github":
            continue
        if stripped:
            ran.append(stripped)
    return ran


def _git(repo: Path, *args: str, env: dict[str, str]) -> str:
    done = subprocess.run(["git", "-C", str(repo), *args], check=True, env=env, text=True, capture_output=True)
    return done.stdout.strip()


def test_codeup_zero_checks_do_not_stop_the_merge() -> None:
    """Codeup records CI and does not wait for a check to register.

    Section 3 stops after max_wait when checkCount stays 0. That command is
    unguarded, so a Codeup merge request with no CI is stopped.
    """

    ran = "\n".join(_codeup_runs(_section(MERGE_SKILL.read_text(), "========== 3. Wait for CI", "========== 4. Attribute")))
    assert "checkCount == 0" not in ran, ran
    assert "no check registered" not in ran, ran


def test_codeup_flaky_rerun_does_not_call_gh() -> None:
    """A flaky Codeup check is not rerun with gh.

    Section 4 tells every host to run `gh run rerun`. Codeup failures have no
    GitHub run id, and gh is the wrong host.
    """

    ran = _codeup_runs(_section(MERGE_SKILL.read_text(), "========== 4. Attribute", "========== 5. Fix"))
    assert not any(line.startswith("`gh ") or "gh run rerun" in line for line in ran), ran


def test_codeup_final_retake_does_not_call_github() -> None:
    """The merge retake must stay on pr_host for Codeup.

    Section 6 runs github_review_lane.py with no host guard. That lane calls gh.
    """

    hits: list[str] = []
    for start, end in (
        ("========== 2. Clear review", "========== 3. Wait"),
        ("========== 6. Merge", "========== OUTPUT"),
    ):
        ran = _codeup_runs(_section(MERGE_SKILL.read_text(), start, end))
        hits.extend(line for line in ran if "github_review_lane.py" in line)
    assert hits == [], hits


def test_codeup_release_merge_does_not_pass_admin() -> None:
    """A Codeup release with remaining checks merges with a note and without --admin.

    CodeupHost.merge rejects --admin before it registers PASS, so the unguarded
    else-branch command cannot merge.
    """

    sys.path.insert(0, str(REPO / "skills/_bus/cli_extensions"))
    from pr_host_codeup import CodeupHost  # noqa: E402
    from pr_host_common import HostError  # noqa: E402

    host = CodeupHost.__new__(CodeupHost)
    with pytest.raises(HostError, match="没有 admin"):
        host.merge("1", "a" * 40, True, "local verification recorded")

    ran = _codeup_runs(_section(MERGE_SKILL.read_text(), "========== 6. Merge", "========== OUTPUT"))
    admin = [line for line in ran if "pr_host merge" in line and "--admin" in line]
    assert admin == [], admin


def test_codeup_review_metrics_do_not_call_gh(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """The post-merge ledger for a Codeup origin does not query GitHub."""

    sys.path.insert(0, str(REPO / "skills/merge-pr/scripts"))
    import review_metrics as rm  # noqa: E402

    repo = tmp_path / "repo"
    repo.mkdir()
    env = os.environ.copy()
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
    subprocess.run(["git", "init", "-q", "-b", "feat", str(repo)], check=True, env=env)
    subprocess.run(
        ["git", "-C", str(repo), "remote", "add", "origin", "https://codeup.aliyun.com/org/repo.git"],
        check=True,
        env=env,
    )
    calls: list[list[str]] = []

    def fake_run(argv, **kwargs):
        calls.append(list(argv))

        class Result:
            returncode = 1
            stdout = ""
            stderr = "stub"

        return Result()

    monkeypatch.setattr(rm.subprocess, "run", fake_run)
    rm.main(["--pr", "7", "--repo", str(repo), "--applied", "0", "--rejected", "0"])
    assert [argv for argv in calls if argv and argv[0] == "gh"] == []


def test_pr_preflight_base_matches_the_live_origin_head(tmp_path: Path) -> None:
    """The published base is origin's advertised HEAD, not a stale local symbolic-ref.

    pr's preflight keeps a set symbolic-ref and passes it as the explicit rebase
    and create base. rebase_base --refresh-default would select the advertised
    HEAD, but an explicit base bypasses that refresh.
    """

    env = os.environ.copy()
    env.update(
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_AUTHOR_NAME="probe",
        GIT_AUTHOR_EMAIL="probe@example.com",
        GIT_COMMITTER_NAME="probe",
        GIT_COMMITTER_EMAIL="probe@example.com",
    )
    seed, bare = tmp_path / "seed", tmp_path / "origin.git"
    _git_init = subprocess.run(["git", "init", "-q", "-b", "master", str(seed)], check=True, env=env)
    del _git_init
    (seed / "marker").write_text("master\n")
    subprocess.run(["git", "-C", str(seed), "add", "marker"], check=True, env=env)
    subprocess.run(["git", "-C", str(seed), "commit", "-qm", "master"], check=True, env=env)
    subprocess.run(["git", "-C", str(seed), "checkout", "-qb", "main"], check=True, env=env)
    (seed / "marker").write_text("main\n")
    subprocess.run(["git", "-C", str(seed), "commit", "-qam", "main"], check=True, env=env)
    subprocess.run(["git", "init", "-q", "--bare", str(bare)], check=True, env=env)
    subprocess.run(["git", "-C", str(seed), "remote", "add", "origin", str(bare)], check=True, env=env)
    subprocess.run(["git", "-C", str(seed), "push", "-q", "origin", "master", "main"], check=True, env=env)
    subprocess.run(["git", "--git-dir", str(bare), "symbolic-ref", "HEAD", "refs/heads/main"], check=True, env=env)

    clone = tmp_path / "clone"
    subprocess.run(["git", "clone", "-q", str(bare), str(clone)], check=True, env=env)
    subprocess.run(
        ["git", "-C", str(clone), "symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/master"],
        check=True,
        env=env,
    )

    skill = PR_SKILL.read_text()
    fetch_at = skill.index("git fetch origin")
    resolver_at = skill.index("rebase_base.py", fetch_at)
    rebase_at = skill.index("rebase-origin-main --base origin/<base_branch>")
    create_at = skill.index("pr_host create --branch <branch> --base <base_branch>")
    assert fetch_at < resolver_at < rebase_at < create_at
    resolver_line = next(line for line in skill.splitlines() if "rebase_base.py" in line)
    assert "--refresh-default" in resolver_line and "--base " not in resolver_line

    selected = _git(clone, "symbolic-ref", "--short", "refs/remotes/origin/HEAD", env=env)
    stale = _git(clone, "rev-parse", f"{selected}", env=env)
    live = subprocess.run(
        [sys.executable, str(REBASE_BASE), "--repo", str(clone), "--refresh-default"],
        check=True,
        env=env,
        text=True,
        capture_output=True,
    ).stdout.strip()
    assert stale != live
    assert _git(clone, "symbolic-ref", "refs/remotes/origin/HEAD", env=env) == "refs/remotes/origin/main"
