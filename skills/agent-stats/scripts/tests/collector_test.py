"""Recorded PR #295 proves canonical root credit without semantic deduplication."""

import json
import subprocess
import sys
from pathlib import Path

CLI = Path(__file__).resolve().parents[1] / "agent_stats.py"
FIXTURE = Path(__file__).with_name("fixtures") / "pr295"


def call(*args):
    return subprocess.run(
        [sys.executable, str(CLI), *map(str, args)], text=True, capture_output=True
    )


def test_recorded_reviews_use_triage_root_credits_and_preserve_unknowns(tmp_path):
    args = (
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        FIXTURE,
    )
    result = call(*args)
    assert result.returncode == 0, result.stderr
    assert call(*args).returncode == 0
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    reviews = [x for x in rows if x["kind"] == "review"]
    assert len(reviews) == 4
    roots = [x for x in rows if x["kind"] == "root_cause"]
    assert len(roots) == 2
    assert [(r["data"]["outcome"], r["data"]["reviewers"]) for r in roots] == [
        ("APPLY", ["clauderev5"]),
        ("APPLY", ["clauderev5", "codexrevf"]),
    ]
    assert all(x["agent"]["model"] == "unknown" for x in rows)
    assert call("validate", tmp_path / "stats/events.jsonl").returncode == 0
    result = call("report", "--store", tmp_path / "stats", "--json")
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    credits = {x["agent"]: x for x in report["leaderboard"]}
    assert credits["clauderev5"]["unique_issues"] == 1
    assert credits["clauderev5"]["shared_issues"] == 1
    assert credits["codexrevf"]["unique_issues"] == 0
    assert credits["codexrevf"]["shared_issues"] == 1
    assert credits["clauderev5"]["precision"] == 1.0
    assert report["bottlenecks"]["unmeasured"]


def test_gate_deduplicates_copies_and_does_not_guess_failure_cause(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    gate = {
        "head": "a" * 40,
        "exit_code": 1,
        "duration_ms": 12500,
        "coverage_ms": None,
        "selection": {"full": True},
        "results": [
            {
                "name": "python",
                "exit_code": 1,
                "duration_ms": 11000,
                "stdout": "FAILED tests/test_x.py::test_y - bad",
                "stderr": "",
            }
        ],
    }
    (root / "gate.json").write_text(json.dumps(gate))
    (root / "copy.json").write_text(json.dumps(gate))
    result = call(
        "collect", "--store", tmp_path / "stats", "--task", "pr-7", "--artifacts", root
    )
    assert result.returncode == 0, result.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    runs = [r for r in rows if r["kind"] == "run"]
    assert len(runs) == 1
    assert runs[0]["data"]["result"] == "FAIL"
    assert runs[0]["data"]["duration_ms"] == 12500
    assert runs[0]["data"]["failure_cause"] is None


def test_conflicting_review_copies_fail_without_partial_import(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    original = (FIXTURE / "clauderev5/round-02/review.md").read_text()
    (root / "review.md").write_text(original)
    (root / "second.md").write_text(
        original.replace("**判定**：Ready", "**判定**：Abandon")
    )
    result = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert result.returncode == 1
    assert not (tmp_path / "stats/events.jsonl").exists()


def test_unknown_structured_source_lane_fails_closed(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    (root / "triage.md").write_text(
        "**模式**：pr\n\n## APPLY\n\n- C1 — root\n\n## Root cause provenance\n\n| claim_id | outcome | sources |\n|---|---|---|\n| C1 | APPLY | ghost@1:1 |\n"
    )
    result = call(
        "collect", "--store", tmp_path / "stats", "--task", "pr-1", "--artifacts", root
    )
    assert result.returncode == 1
    assert not (tmp_path / "stats/events.jsonl").exists()


def test_archived_triage_copy_with_extra_validation_does_not_double_root_credit(
    tmp_path,
):
    import shutil

    root = tmp_path / "artifacts"
    shutil.copytree(FIXTURE, root)
    copy = root / "evidence/author-triage.md"
    copy.parent.mkdir()
    copy.write_text(
        (root / "author/triage.md").read_text()
        + "\n## Later validation\n\nAll probes pass.\n"
    )
    r = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert r.returncode == 0, r.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    assert len([r for r in rows if r["kind"] == "root_cause"]) == 2


def test_archived_gate_and_e2e_formats_preserve_results_without_inventing_elapsed(
    tmp_path,
):
    root = tmp_path / "artifacts"
    root.mkdir()
    (root / "gate.md").write_text(
        "# Gate-and-Fix Round\n\n- Schema: `2`\n- Round: `2`\n- Base: `306b177332aaefd3addfbbb6c6f1cafb1b3b8a7c`\n- Head: `ad9dc509e1a63a8ecd99cb4136d01e54a1ce9266`\n- Outcome: `PASS`\n\n## Gate results\n\n### lint — PASS\n\n- Exit code: `0`\n- Duration: `124992ms`\n"
    )
    (root / "e2e.md").write_text(
        "# E2E TUI Run\n\n- Commit: `8af9163bdadc22341a998869b30e41f6be82beb1`\n\n## Summary\n\n```text\nERRPIPE-01 PASS\nVerdict: GREEN\n```\n"
    )
    (root / "live.json").write_text(
        json.dumps(
            {
                "result": "PASS",
                "head": "ad9dc509e1a63a8ecd99cb4136d01e54a1ce9266",
                "provider": "codex",
                "recovered_request": {"stage": "replied"},
            }
        )
    )
    r = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert r.returncode == 0, r.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    runs = {r["data"]["kind"]: r["data"] for r in rows if r["kind"] == "run"}
    assert set(runs) == {"gate", "e2e", "live"}
    assert runs["gate"]["lanes"] == [
        {"name": "lint", "result": "PASS", "duration_ms": 124992}
    ]
    assert all(x["duration_ms"] is None for x in runs.values())


def test_collection_preserves_input_bytes_after_original_artifacts_change(tmp_path):
    import hashlib

    root = tmp_path / "artifacts"
    root.mkdir()
    path = root / "review.md"
    path.write_bytes((FIXTURE / "clauderev5/round-02/review.md").read_bytes())
    r = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert r.returncode == 0, r.stderr
    path.write_text("evidence removed after worktree close")
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    for row in rows:
        for src in row["sources"]:
            assert (
                hashlib.sha256(Path(src["path"]).read_bytes()).hexdigest()
                == src["sha256"]
            )


def test_sanitized_source_lane_annotations_do_not_drop_canonical_roots_or_supply_models(
    tmp_path,
):
    root = tmp_path / "artifacts"
    root.mkdir()
    (root / "sanitized.md").write_text(
        "> **来源**：mode=pr | lane=kimirevc（provider: kimi；model: K3.1 Pro） | round=1 | file=/gone/review.md | finding-ids=1,2\n"
    )
    (root / "triage.md").write_text(
        "**模式**：pr\n\n## APPLY\n\n- K1 / kimirevc1 — SUPPORTED root\n"
    )
    r = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-284",
        "--artifacts",
        root,
    )
    assert r.returncode == 0, r.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    roots = [r for r in rows if r["kind"] == "root_cause"]
    assert len(roots) == 1
    assert roots[0]["data"]["reviewers"] == ["kimirevc"]
    assert roots[0]["data"]["source_refs"] == [
        {"reviewer": "kimirevc", "round": 1, "finding_id": "1", "scope_hash": None}
    ]
    report = json.loads(call("report", "--store", tmp_path / "stats", "--json").stdout)
    assert report["leaderboard"][0]["model"] == "unknown"
    assert report["model_leaderboard"][0]["unique_issues"] == 1


def test_backfill_command_replays_recorded_metadata_and_is_idempotent(tmp_path):
    import shutil

    artifacts = tmp_path / "artifacts"
    shutil.copytree(FIXTURE, artifacts / "pr-295")
    gh = tmp_path / "gh"
    gh.mkdir()
    (gh / "295.json").write_text(
        json.dumps(
            dict(
                number=295,
                title="Fixture",
                url="https://github.com/example/repo/pull/295",
                state="MERGED",
                headRefName="fix/codex-rollout-final-answer",
                headRefOid="ad9dc509e1a63a8ecd99cb4136d01e54a1ce9266",
                createdAt="2026-11-02T01:00:00Z",
                mergedAt="2026-11-02T03:00:00Z",
                mergeCommit={"oid": "b" * 40},
                author={"login": "fixture"},
                commits=[{"oid": "a" * 40}],
                additions=10,
                deletions=3,
                changedFiles=2,
            )
        )
    )
    args = (
        "backfill",
        "--store",
        tmp_path / "stats",
        "--prs",
        "295",
        "--artifacts-root",
        artifacts,
        "--gh-dir",
        gh,
    )
    first = call(*args)
    assert first.returncode == 0, first.stderr
    assert json.loads(first.stdout)["appended"] > 0
    second = call(*args)
    assert second.returncode == 0, second.stderr
    assert json.loads(second.stdout)["appended"] == 0
    rows = json.loads(call("report", "--store", tmp_path / "stats", "--json").stdout)[
        "prs"
    ]["pr-295"]
    pr = [r["data"] for r in rows if r["kind"] == "pr"][0]
    assert (pr["commit_count"], pr["additions"], pr["deletions"], pr["files"]) == (
        1,
        10,
        3,
        2,
    )


def test_explicit_single_claim_legacy_ledger_uses_its_one_recorded_source(tmp_path):
    import shutil

    root = tmp_path / "artifacts"
    root.mkdir()
    shutil.copyfile(FIXTURE / "codexrevf/round-01/review.md", root / "review.md")
    (root / "triage.md").write_text(
        "**模式**：pr\nTarget: PR #295 / current HEAD 45fffccda08b626b111afbe61aa77b192c0c9f1d.\nSelf-verified, one canonical claim from codexrevf round 1; no cross-lane conflict.\n\nAPPLY-BEHAVIOR (partial): existing projection root.\nREJECT：无\nFLAGGED：无\n"
    )
    r = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert r.returncode == 0, r.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    roots = [r for r in rows if r["kind"] == "root_cause"]
    assert len(roots) == 1
    assert roots[0]["data"]["outcome"] == "APPLY"
    assert roots[0]["data"]["source_refs"] == [
        {"reviewer": "codexrevf", "round": 1, "finding_id": "1", "scope_hash": None}
    ]


def test_gate_wrapper_and_its_referenced_runner_count_as_one_gate(tmp_path):
    import base64

    root = tmp_path / "artifacts"
    root.mkdir()
    name = "run-" + ("c" * 32) + ".json"
    gate = dict(
        head="a" * 40,
        exit_code=1,
        duration_ms=12500,
        coverage_ms=None,
        selection={"full": True},
        results=[
            dict(name="python", exit_code=1, duration_ms=11000, stdout="", stderr="")
        ],
    )
    (root / name).write_text(json.dumps(gate))
    log = base64.b64encode(
        ("Test report: /deleted/temp/" + name + "\nElapsed: 12.500s\n").encode()
    ).decode()
    (root / "round.md").write_text(
        "# Gate-and-Fix Round\n\n- Schema: `2`\n- Round: `2`\n- Base: `"
        + ("b" * 40)
        + "`\n- Head: `"
        + ("a" * 40)
        + "`\n- Outcome: `FAIL`\n\n## Gate results\n\n### repository-gate — FAIL\n\n- Exit code: `1`\n- Duration: `12500ms`\n\n#### stdout\n\n```base64\n"
        + log
        + "\n```\n"
    )
    r = call(
        "collect", "--store", tmp_path / "stats", "--task", "pr-1", "--artifacts", root
    )
    assert r.returncode == 0, r.stderr
    report = json.loads(call("report", "--store", tmp_path / "stats", "--json").stdout)
    assert sum(r["gate_fail"] for r in report["leaderboard"]) == 1
    assert report["bottlenecks"]["measured"]["gate_elapsed_ms"] == 12500


def test_a_reviewer_mentioned_only_as_later_verifier_is_not_a_shared_finder(tmp_path):
    import shutil

    root = tmp_path / "artifacts"
    root.mkdir()
    shutil.copyfile(FIXTURE / "codexrevf/round-01/review.md", root / "a.md")
    shutil.copyfile(FIXTURE / "clauderev5/round-02/review.md", root / "b.md")
    (root / "triage.md").write_text(
        "**模式**：pr\n\n## APPLY\n\n- C1 / codexrevf1 — SUPPORTED root; clauderev5 later verified the repair.\n"
    )
    r = call(
        "collect",
        "--store",
        tmp_path / "stats",
        "--task",
        "pr-295",
        "--artifacts",
        root,
    )
    assert r.returncode == 0, r.stderr
    rows = [
        json.loads(x)
        for x in (tmp_path / "stats/events.jsonl").read_text().splitlines()
    ]
    assert [r["data"]["reviewers"] for r in rows if r["kind"] == "root_cause"] == [
        ["codexrevf"]
    ]


def test_source_rewrite_between_parse_and_hash_fails_closed(tmp_path, monkeypatch):
    import pytest

    sys.path.insert(0, str(CLI.parent))
    import stats_collect

    root = tmp_path / "artifacts"
    root.mkdir()
    review = root / "review.md"
    original = (FIXTURE / "clauderev5/round-02/review.md").read_text()
    review.write_text(original)
    real_source = stats_collect.source

    def racing_source(path, locator="$"):
        if Path(path) == review:
            review.write_text(original.replace("**判定**：Ready", "**判定**：Abandon"))
        return real_source(path, locator)

    monkeypatch.setattr(stats_collect, "source", racing_source)
    with pytest.raises(ValueError, match="changed"):
        stats_collect.collect("pr-295", root)


def test_current_gate_artifact_uses_its_canonical_verifier(tmp_path, monkeypatch):
    import importlib.util

    path = CLI.parents[2] / "gate-and-fix/scripts/gate_artifact.py"
    spec = importlib.util.spec_from_file_location("stats_gate_fixture", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    text = module.render_round(
        round_number=1,
        base="b" * 40,
        head="a" * 40,
        changed_files=["file.py"],
        results=[module.GateResult("lint", ("true",), 0, 10, "ok", "")],
    )
    root = tmp_path / "artifacts"
    root.mkdir()
    artifact = root / "round.md"
    artifact.write_text(text.replace("- Bytes: `2`", "- Bytes: `99`"))
    r = call(
        "collect", "--store", tmp_path / "stats", "--task", "pr-1", "--artifacts", root
    )
    assert r.returncode == 1
    assert not (tmp_path / "stats/events.jsonl").exists()
    artifact.write_text(text)
    sys.path.insert(0, str(CLI.parent))
    from stats_collect import collect

    actual_run = subprocess.run
    operations = []

    def observed_run(argv, *args, **kwargs):
        if len(argv) > 2 and Path(argv[1]).name == "gate_and_fix.py":
            operations.append(argv[2])
        return actual_run(argv, *args, **kwargs)

    monkeypatch.setattr(subprocess, "run", observed_run)
    rows = collect("pr-1", root)
    assert next(row for row in rows if row["kind"] == "run")["data"]["result"] == "PASS"
    assert "verify" in operations and "show" in operations


def test_author_assignments_link_exact_pr_or_branch_before_merge(tmp_path):
    sys.path.insert(0, str(CLI.parent))
    from stats_collect import collect
    from stats_report import report
    from stats_schema import record, source, unknown

    root = tmp_path / "artifacts"
    root.mkdir()
    gh = tmp_path / "gh.json"
    gh.write_text(
        json.dumps(
            dict(
                number=7,
                title="Recorded PR",
                url="https://example.org/7",
                state="MERGED",
                headRefName="feat/tracked",
                headRefOid="a" * 40,
                createdAt="2026-11-02T01:00:00Z",
                mergedAt="2026-11-02T03:00:00Z",
                mergeCommit={"oid": "b" * 40},
                author={"login": "developer"},
                commits=[{"oid": "a" * 40}],
                additions=10,
                deletions=2,
                changedFiles=3,
            )
        )
    )

    def assigned(task, name, role="author", at="2026-11-02T02:00:00Z"):
        actor = dict(
            id=name, name=name, provider="recorded-provider", model="recorded-model"
        )
        return record(
            "assignment",
            task,
            role,
            actor,
            unknown(),
            [source(gh)],
            dict(message_id=None, room_id=568, workspace_snapshot=None),
            at,
        )

    history = [
        assigned("feat/tracked", "first"),
        assigned("pr-7", "second"),
        assigned("7", "third"),
        assigned("other-task", "unrelated"),
        assigned("pr-7", "late", at="2026-11-02T04:00:00Z"),
        assigned("pr-7", "reviewer", "reviewer"),
    ]
    rows = collect("pr-7", root, gh=gh, history=history)
    pr = next(r for r in rows if r["kind"] == "pr")
    assert pr["data"]["authors"] == ["first", "second", "third"]
    assert len(pr["sources"]) == 4
    board = report(history + rows)["model_leaderboard"]
    assert next(r for r in board if r["role"] == "author")["authored_prs"] == 3


def test_completed_rebase_receipts_are_counted_once_not_workflow_mentions(tmp_path):
    sys.path.insert(0, str(CLI.parent))
    from stats_collect import collect
    from stats_report import report

    receipt = dict(
        base="a" * 40,
        old_head="b" * 40,
        head="c" * 40,
        commits_replayed=15,
        skipped=1,
        foreign=2,
        own_versions_replaced=0,
        conflicts=["recorded-path"],
        resolution="recorded resolution",
        dropped_shared_infra=[],
    )
    for name in ("first", "copy"):
        folder = tmp_path / name
        folder.mkdir()
        (folder / "result.json").write_text(json.dumps(receipt))
    rows = collect("pr-7", tmp_path)
    assert len([r for r in rows if r["kind"] == "rebase"]) == 1
    assert report(rows)["rebases_by_task"] == {"pr-7": 1}


def test_unknown_runner_identity_does_not_imply_one_unique_agent(tmp_path):
    sys.path.insert(0, str(CLI.parent))
    from stats_collect import collect
    from stats_report import report

    (tmp_path / "run.json").write_text(
        json.dumps(dict(head="a" * 40, exit_code=0, duration_ms=12, results=[]))
    )
    model = report(collect("pr-7", tmp_path))["model_leaderboard"][0]
    assert model["gate_pass"] == 1
    assert model["agents"] is None


def test_recorded_bare_canonical_roots_keep_exact_legacy_source_ids(tmp_path):
    for pr, expected in [
        (284, {"A": ["codexrevd", "kimirevc"], "B": []}),
        (285, {"B1": ["clauderev4"], "B2": ["clauderev4"]}),
    ]:
        fixture = FIXTURE.parent / f"legacy-pr{pr}"
        store = tmp_path / str(pr)
        result = call(
            "collect", "--store", store, "--task", f"pr-{pr}", "--artifacts", fixture
        )
        assert result.returncode == 0, result.stderr
        rows = [
            json.loads(x) for x in (store / "events.jsonl").read_text().splitlines()
        ]
        roots = {
            r["data"]["claim_id"]: r["data"] for r in rows if r["kind"] == "root_cause"
        }
        assert {cid: r["reviewers"] for cid, r in roots.items()} == expected
        if pr == 284:
            assert {
                (r["reviewer"], r["round"], r["finding_id"])
                for r in roots["A"]["source_refs"]
            } == {("codexrevd", 4, "1"), ("kimirevc", 4, "1")}
        else:
            assert [
                (r["round"], r["finding_id"]) for r in roots["B1"]["source_refs"]
            ] == [(1, "1")]
            assert [
                (r["round"], r["finding_id"]) for r in roots["B2"]["source_refs"]
            ] == [(1, "2")]
        report = json.loads(call("report", "--store", store, "--json").stdout)
        credits = {r["agent"]: r for r in report["leaderboard"]}
        if pr == 284:
            assert (
                credits["kimirevc"]["shared_issues"]
                == credits["codexrevd"]["shared_issues"]
                == 1
            )
            rendered = call("report", "--store", store).stdout
            assert "root B APPLY: reviewer attribution unavailable" in rendered
        else:
            assert credits["clauderev4"]["unique_issues"] == 2


def test_schema_two_conflicts_reject_before_appending_any_records(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    for aggregate, lane, exitcode in [
        ("PASS", "FAIL", 1),
        ("FAIL", "PASS", 0),
        ("PASS", "PASS", 1),
        ("FAIL", "FAIL", 0),
    ]:
        text = (
            "# Gate-and-Fix Round\n\n- Schema: `2`\n- Round: `1`\n- Base: `"
            + "b" * 40
            + "`\n- Head: `"
            + "a" * 40
            + f"`\n- Outcome: `{aggregate}`\n\n## Gate results\n\n### repository-gate — {lane}\n\n- Exit code: `{exitcode}`\n- Duration: `1000ms`\n"
        )
        (root / "round.md").write_text(text)
        store = tmp_path / (aggregate + lane + str(exitcode))
        result = call(
            "collect", "--store", store, "--task", "pr-1", "--artifacts", root
        )
        assert result.returncode == 1, (
            aggregate,
            lane,
            exitcode,
            result.stdout,
            result.stderr,
        )
        assert not (store / "events.jsonl").exists()


def test_passing_wrapper_cannot_hide_its_explicit_failed_runner(tmp_path):
    import base64

    root = tmp_path / "artifacts"
    root.mkdir()
    name = "run-" + "d" * 32 + ".json"
    runner = dict(head="a" * 40, exit_code=1, duration_ms=1000, results=[])
    (root / name).write_text(json.dumps(runner))
    log = base64.b64encode(("Test report: /archived/" + name + "\n").encode()).decode()
    wrapper = (
        "# Gate-and-Fix Round\n\n- Schema: `2`\n- Round: `1`\n- Base: `"
        + "b" * 40
        + "`\n- Head: `"
        + "a" * 40
        + "`\n- Outcome: `PASS`\n\n## Gate results\n\n### repository-gate — PASS\n\n- Exit code: `0`\n- Duration: `1000ms`\n\n#### stdout\n\n```base64\n"
        + log
        + "\n```\n"
    )
    (root / "round.md").write_text(wrapper)
    store = tmp_path / "stats"
    result = call("collect", "--store", store, "--task", "pr-1", "--artifacts", root)
    assert result.returncode == 1
    assert "outcome disagrees" in result.stderr
    assert not (store / "events.jsonl").exists()

    runner["exit_code"] = 0
    (root / name).write_text(json.dumps(runner))
    result = call("collect", "--store", store, "--task", "pr-1", "--artifacts", root)
    assert result.returncode == 0, result.stderr
    value = json.loads(call("report", "--store", store, "--json").stdout)
    assert sum(row["gate_pass"] for row in value["leaderboard"]) == 1


def test_current_review_must_pass_the_canonical_section_boundary(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    original = (FIXTURE / "codexrevf/round-01/review.md").read_text()
    (root / "review.md").write_text(
        original.replace("## 本轮探索区域", "## Extra section")
    )
    store = tmp_path / "stats"
    result = call("collect", "--store", store, "--task", "pr-1", "--artifacts", root)
    assert result.returncode == 1
    assert "section" in result.stderr
    assert not (store / "events.jsonl").exists()
