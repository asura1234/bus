"""Git and ordered source timestamps must prove an applied root escaped Ready."""

import json
import subprocess
import sys
from pathlib import Path

CLI = Path(__file__).resolve().parents[1] / "agent_stats.py"
FIXTURE = Path(__file__).with_name("fixtures") / "pr295"


def observed_completion(store, artifact, lane, at):
    # Recorded machine phase fixture: completion binds the exact artifact hash.
    sys.path.insert(0, str(CLI.parent))
    from stats_schema import record, source, unknown
    from stats_store import append

    src = source(artifact)
    base = dict(
        phase="in_review",
        artifact_sha=None,
        transition="start",
        start_id=None,
        duration_ms=None,
    )
    start = record(
        "phase", "pr-1", "reviewer", unknown(lane), unknown(), [src], base, at
    )
    end = record(
        "phase",
        "pr-1",
        "reviewer",
        unknown(lane),
        unknown(),
        [src],
        dict(
            phase="in_review",
            artifact_sha=src["sha256"],
            transition="end",
            start_id=start["id"],
            duration_ms=0,
        ),
        at,
    )
    append(store, [start, end])


def git(repo, *args):
    return subprocess.run(
        ["git", "-C", str(repo), *args], check=True, text=True, capture_output=True
    ).stdout.strip()


def test_escapes_require_applied_root_later_time_and_unchanged_cited_code(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-b", "feature")
    git(repo, "config", "user.email", "fixture@example.invalid")
    git(repo, "config", "user.name", "Fixture")
    code = repo / "src/messaging/model/status.rs"
    code.parent.mkdir(parents=True)
    code.write_text("// existing code\n" * 70)
    git(repo, "add", "src")
    git(repo, "commit", "-m", "base")
    first = git(repo, "rev-parse", "HEAD")
    (repo / "unrelated").write_text("new file")
    git(repo, "add", "unrelated")
    git(repo, "commit", "-m", "later")
    later = git(repo, "rev-parse", "HEAD")
    root = tmp_path / "artifacts"
    root.mkdir()
    ready = (
        (FIXTURE / "clauderev5/round-02/review.md")
        .read_text()
        .replace("clauderev5", "early")
        .replace("fix/codex-rollout-final-answer @ ad9dc509", "feature @ " + first)
    )
    ready = ready.replace(
        "**日期**：2026-11-02",
        "**日期**：2026-11-02\n**Completed at**：2026-11-02T01:00:00Z",
    )
    later_review = (
        (FIXTURE / "codexrevf/round-01/review.md")
        .read_text()
        .replace(
            "fix/codex-rollout-final-answer @ 45fffccda08b626b111afbe61aa77b192c0c9f1d",
            "feature @ " + later,
        )
    )
    later_review = later_review.replace(
        "**日期**：2026-11-02",
        "**日期**：2026-11-02\n**Completed at**：2026-11-02T02:00:00Z",
    )
    (root / "ready.md").write_text(ready)
    (root / "later.md").write_text(later_review)
    (root / "triage.md").write_text(
        "**模式**：pr\nTarget head "
        + later
        + "\n\n## APPLY\n\n- C1 — existing root\n\n## Root cause provenance\n\n| claim_id | outcome | sources |\n|---|---|---|\n| C1 | APPLY | codexrevf@1:1 |\n"
    )

    def run(store, ordered=True):
        observed_completion(store, root / "ready.md", "early", "2026-11-02T01:00:00Z")
        if ordered:
            observed_completion(
                store, root / "later.md", "codexrevf", "2026-11-02T02:00:00Z"
            )
        r = subprocess.run(
            [
                sys.executable,
                str(CLI),
                "collect",
                "--store",
                str(store),
                "--task",
                "pr-1",
                "--artifacts",
                str(root),
                "--repo",
                str(repo),
            ],
            text=True,
            capture_output=True,
        )
        assert r.returncode == 0, r.stderr
        return [
            json.loads(x) for x in (store / "events.jsonl").read_text().splitlines()
        ]

    escapes = [x for x in run(tmp_path / "positive") if x["kind"] == "escape"]
    assert len(escapes) == 1
    assert escapes[0]["agent"]["name"] == "early"
    assert escapes[0]["data"]["ready_head"] == first
    # Without ordered completion evidence the same code is not enough.
    (root / "later.md").write_text(
        later_review.replace("**Completed at**：2026-11-02T02:00:00Z\n", "")
    )
    assert not [
        x for x in run(tmp_path / "unordered", ordered=False) if x["kind"] == "escape"
    ]
    (root / "later.md").write_text(later_review)
    code.write_text("// modified code\n" * 70)
    git(repo, "add", "src")
    git(repo, "commit", "-m", "changed code")
    changed = git(repo, "rev-parse", "HEAD")
    for name in ["later.md", "triage.md"]:
        p = root / name
        p.write_text(p.read_text().replace(later, changed))
    assert not [x for x in run(tmp_path / "changed") if x["kind"] == "escape"]


def test_machine_linked_failed_gate_and_live_roots_can_escape_ready(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-b", "feature")
    git(repo, "config", "user.email", "fixture@example.invalid")
    git(repo, "config", "user.name", "Fixture")
    (repo / "code.rs").write_text("existing defect\n")
    git(repo, "add", "code.rs")
    git(repo, "commit", "-m", "base")
    head = git(repo, "rev-parse", "HEAD")
    root = tmp_path / "artifacts"
    root.mkdir()
    ready = (
        (FIXTURE / "clauderev5/round-02/review.md")
        .read_text()
        .replace("clauderev5", "early")
        .replace("fix/codex-rollout-final-answer @ ad9dc509", "feature @ " + head)
    )
    ready = ready.replace(
        "**日期**：2026-11-02",
        "**日期**：2026-11-02\n**Completed at**：2026-11-02T01:00:00Z",
    )
    (root / "review.md").write_text(ready)
    for kind in ["gate", "live"]:
        (root / "failed.json").write_text(
            json.dumps(
                dict(
                    kind=kind,
                    head=head,
                    result="FAIL",
                    duration_ms=1000,
                    failing_test_ids=["tests/x.py::test_y"],
                    started_at="2026-11-02T02:00:00Z",
                    finished_at="2026-11-02T02:00:01Z",
                )
            )
        )
        (root / "triage.md").write_text(
            "**模式**：pr\nTarget head "
            + head
            + "\n\n## APPLY\n\n- C1 — code.rs:1; proved by failed.json\n\n## Root cause provenance\n\n| claim_id | outcome | sources |\n|---|---|---|\n| C1 | APPLY | run@failed.json#tests/x.py::test_y |\n"
        )
        store = tmp_path / kind
        observed_completion(store, root / "review.md", "early", "2026-11-02T01:00:00Z")
        r = subprocess.run(
            [
                sys.executable,
                str(CLI),
                "collect",
                "--store",
                str(store),
                "--task",
                "pr-1",
                "--artifacts",
                str(root),
                "--repo",
                str(repo),
            ],
            text=True,
            capture_output=True,
        )
        assert r.returncode == 0, r.stderr
        rows = [
            json.loads(x) for x in (store / "events.jsonl").read_text().splitlines()
        ]
        escaped = [r for r in rows if r["kind"] == "escape"]
        assert len(escaped) == 1
        assert escaped[0]["data"]["detected_by"] == [kind]
