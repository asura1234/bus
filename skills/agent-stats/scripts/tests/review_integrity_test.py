"""Private fixture probes for append-only review statistics integrity."""

import hashlib
import json
import re
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[4]
SCRIPTS = ROOT / "skills/agent-stats/scripts"
CLI = SCRIPTS / "agent_stats.py"
FIXTURE = SCRIPTS / "tests/fixtures/pr295"
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(SCRIPTS / "tests"))
from cli_extensions.review_artifact_parser import parse_review_artifact  # noqa: E402
from escapes_test import observed_completion  # noqa: E402


def ready_template():
    text = (
        (FIXTURE / "clauderev5/round-02/review.md")
        .read_text()
        .replace("# Review Round 2", "# Review Round 1")
    )
    return re.sub(
        r"(## 前轮问题核销\n).*?(?=## 新问题与建议)", r"\1\n无。\n\n", text, flags=re.S
    )


def call(*args):
    return subprocess.run(
        [sys.executable, str(CLI), *map(str, args)], text=True, capture_output=True
    )


def collect(root, store, repo=None):
    argv = ["collect", "--store", store, "--task", "pr-1", "--artifacts", root]
    if repo:
        argv += ["--repo", repo]
    return call(*argv)


def records(store):
    return [
        json.loads(line) for line in (store / "events.jsonl").read_text().splitlines()
    ]


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def test_conflicting_reimport_is_rejected_before_store_mutation(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    artifact = root / "review.md"
    artifact.write_text(ready_template())
    parse_review_artifact(artifact)
    store = tmp_path / "stats"
    first = collect(root, store)
    assert first.returncode == 0, first.stderr
    assert call("report", "--store", store, "--json").returncode == 0
    before = (store / "events.jsonl").read_bytes()
    artifact.write_text(
        artifact.read_text().replace("**判定**：Ready", "**判定**：Abandon")
    )
    parse_review_artifact(artifact)
    second = collect(root, store)
    after = (store / "events.jsonl").read_bytes()
    reporting = call("report", "--store", store, "--json")
    save(
        tmp_path / "observed.json",
        dict(
            first=first.stdout,
            second_returncode=second.returncode,
            second_stdout=second.stdout,
            second_stderr=second.stderr,
            store_changed=after != before,
            report_returncode=reporting.returncode,
            report_stderr=reporting.stderr,
        ),
    )
    assert second.returncode == 1 and after == before, (
        f"conflicting import returned {second.returncode}, changed store={after != before}; "
        f"following report returned {reporting.returncode}: {reporting.stderr}"
    )


def test_distinct_canonical_review_scopes_are_not_collapsed(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    template = ready_template()
    hashes = []
    for index, path in enumerate(
        [
            "skills/agent-stats/scripts/stats_store.py",
            "skills/agent-stats/scripts/stats_report.py",
        ]
    ):
        manifest = tmp_path / f"scope-{index}.json"
        content = json.dumps(
            dict(files=[path], test_files=[]), sort_keys=True, separators=(",", ":")
        )
        manifest.write_text(content)
        hash_ = hashlib.sha256(content.encode()).hexdigest()
        hashes.append(hash_)
        text = template.replace(
            "**审查者**：",
            f"**范围哈希**：{hash_}\n**范围文件**：{manifest}\n**审查者**：",
        )
        artifact = root / f"scope-{index}.md"
        artifact.write_text(text)
        assert parse_review_artifact(artifact).target["scope_hash"] == hash_
    assert len(set(hashes)) == 2
    store = tmp_path / "stats"
    result = collect(root, store)
    assert result.returncode == 0, result.stderr
    reviews = [row for row in records(store) if row["kind"] == "review"]
    summary = call("report", "--store", store, "--json")
    assert summary.returncode == 0, summary.stderr
    count = sum(
        row["review_rounds"] for row in json.loads(summary.stdout)["leaderboard"]
    )
    save(
        tmp_path / "observed.json",
        dict(
            canonical_scope_hashes=hashes,
            imported_reviews=len(reviews),
            reported_rounds=count,
        ),
    )
    assert len(reviews) == 2 and count == 2, (
        f"two canonical scoped histories became {len(reviews)} records / {count} reported rounds"
    )


def test_late_corroboration_does_not_move_first_detection_past_ready(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    head = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"],
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    ready = ready_template().replace("clauderev5", "ready")
    ready = ready.replace(
        "fix/codex-rollout-final-answer @ ad9dc509", "feature @ " + head
    )
    (root / "ready.md").write_text(ready)
    source = (
        (FIXTURE / "codexrevf/round-01/review.md")
        .read_text()
        .replace(
            "fix/codex-rollout-final-answer @ 45fffccda08b626b111afbe61aa77b192c0c9f1d",
            "feature @ " + head,
        )
    )
    for lane in ["before", "after"]:
        (root / f"{lane}.md").write_text(source.replace("codexrevf", lane))
    triage = root / "triage.md"
    prefix = (
        "**模式**：pr\nTarget head "
        + head
        + "\n\n## APPLY\n\n- C1 — existing root\n\n## Root cause provenance\n\n| claim_id | outcome | sources |\n|---|---|---|\n"
    )
    # Positive control: a genuinely later detection still yields one escape.
    triage.write_text(prefix + "| C1 | APPLY | after@1:1 |\n")
    store = tmp_path / "later-only"
    observed_completion(store, root / "ready.md", "ready", "2026-11-02T02:00:00Z")
    observed_completion(store, root / "after.md", "after", "2026-11-02T03:00:00Z")
    first = collect(root, store, ROOT)
    assert first.returncode == 0, first.stderr
    assert len([r for r in records(store) if r["kind"] == "escape"]) == 1
    # The same canonical root was already detected before Ready, then corroborated later.
    triage.write_text(prefix + "| C1 | APPLY | before@1:1, after@1:1 |\n")
    store = tmp_path / "shared-root"
    for lane, at in [("before", "01"), ("ready", "02"), ("after", "03")]:
        observed_completion(store, root / f"{lane}.md", lane, f"2026-11-02T{at}:00:00Z")
    result = collect(root, store, ROOT)
    assert result.returncode == 0, result.stderr
    escaped = [r for r in records(store) if r["kind"] == "escape"]
    save(
        tmp_path / "observed.json",
        dict(
            positive_control_escapes=1,
            linked_detection_times=["01:00", "03:00"],
            ready_time="02:00",
            escape_events=escaped,
        ),
    )
    assert escaped == [], (
        f"root detected at01:00 was counted after02:00 Ready using03:00 corroboration: {escaped}"
    )

    # Unknown chronology on any linked detector cannot prove a first discovery.
    (root / "failed.json").write_text(
        json.dumps(
            dict(
                kind="gate",
                head=head,
                result="FAIL",
                started_at=None,
                finished_at=None,
                failing_test_ids=["reported-case"],
            )
        )
    )
    triage.write_text(
        prefix + "| C1 | APPLY | after@1:1, run@failed.json#reported-case |\n"
    )
    store = tmp_path / "unknown-linked-clock"
    observed_completion(store, root / "ready.md", "ready", "2026-11-02T02:00:00Z")
    observed_completion(store, root / "after.md", "after", "2026-11-02T03:00:00Z")
    result = collect(root, store, ROOT)
    assert result.returncode == 0, result.stderr
    assert not [r for r in records(store) if r["kind"] == "escape"]

    # A file-scoped Ready cannot establish a PR-wide escape without membership proof.
    triage.write_text(prefix + "| C1 | APPLY | after@1:1 |\n")
    (root / "ready.md").write_text(
        ready.replace(
            "**审查者**：",
            "**范围哈希**："
            + "d" * 64
            + "\n**范围文件**：/recorded/scope.json\n**审查者**：",
        )
    )
    store = tmp_path / "scoped-ready"
    observed_completion(store, root / "ready.md", "ready", "2026-11-02T02:00:00Z")
    observed_completion(store, root / "after.md", "after", "2026-11-02T03:00:00Z")
    result = collect(root, store, ROOT)
    assert result.returncode == 0, result.stderr
    assert not [r for r in records(store) if r["kind"] == "escape"]


def test_missing_wrapped_report_cannot_suppress_an_unrelated_failed_run(tmp_path):
    import base64

    root = tmp_path / "artifacts"
    root.mkdir()
    head = "a" * 40
    unrelated = dict(
        head=head,
        exit_code=1,
        duration_ms=1000,
        results=[
            dict(name="python", exit_code=1, duration_ms=1000, stdout="", stderr="")
        ],
    )
    (root / "unrelated-run-B.json").write_text(json.dumps(unrelated))
    log = base64.b64encode(
        b"Test report: /deleted/run-A.json\nElapsed: 1.000s\n"
    ).decode()
    (root / "round.md").write_text(
        "# Gate-and-Fix Round\n\n- Schema: `2`\n- Round: `1`\n- Base: `"
        + "b" * 40
        + "`\n- Head: `"
        + head
        + "`\n- Outcome: `PASS`\n\n## Gate results\n\n### repository-gate — PASS\n\n"
        "- Exit code: `0`\n- Duration: `1000ms`\n\n#### stdout\n\n```base64\n"
        + log
        + "\n```\n"
    )
    store = tmp_path / "stats"
    result = collect(root, store)
    assert result.returncode == 0, result.stderr
    summary = call("report", "--store", store, "--json")
    assert summary.returncode == 0, summary.stderr
    value = json.loads(summary.stdout)
    passes = sum(row["gate_pass"] for row in value["leaderboard"])
    failures = sum(row["gate_fail"] for row in value["leaderboard"])
    save(
        tmp_path / "observed.json",
        dict(
            wrapper_ref="run-A.json",
            available_report="unrelated-run-B.json",
            available_report_result="FAIL",
            wrapper_result="PASS",
            reported_passes=passes,
            reported_failures=failures,
            stored_runs=[r for r in records(store) if r["kind"] == "run"],
        ),
    )
    assert failures == 1, (
        f"unrelated FAIL disappeared behind missing-run-A PASS wrapper: passes={passes}, failures={failures}"
    )


def test_concurrent_conflicting_imports_leave_one_reportable_review(tmp_path):
    from concurrent.futures import ThreadPoolExecutor

    roots = [tmp_path / "ready", tmp_path / "abandon"]
    for root, verdict in zip(roots, ["Ready", "Abandon"]):
        root.mkdir()
        (root / "review.md").write_text(
            ready_template().replace("**判定**：Ready", "**判定**：" + verdict)
        )
    store = tmp_path / "stats"
    with ThreadPoolExecutor(2) as executor:
        results = list(executor.map(lambda root: collect(root, store), roots))
    assert sorted(r.returncode for r in results) == [0, 1]
    assert len([r for r in records(store) if r["kind"] == "review"]) == 1
    assert call("report", "--store", store, "--json").returncode == 0


def test_ambiguous_scope_source_preserves_root_without_guessing_which_review(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    template = (FIXTURE / "codexrevf/round-01/review.md").read_text()
    for n in (1, 2):
        (root / f"review-{n}.md").write_text(
            template.replace(
                "**审查者**：",
                f"**范围哈希**：{str(n) * 64}\n**范围文件**：/recorded/scope-{n}.json\n**审查者**：",
            )
        )
    (root / "triage.md").write_text(
        "**模式**：pr\n\n## APPLY\n\n- C1 — shared scope namespace\n\n## Root cause provenance\n\n| claim_id | outcome | sources |\n|---|---|---|\n| C1 | APPLY | codexrevf@1:1 |\n"
    )
    store = tmp_path / "stats"
    result = collect(root, store)
    assert result.returncode == 0, result.stderr
    rows = records(store)
    assert len([r for r in rows if r["kind"] == "review"]) == 2
    ref = next(r for r in rows if r["kind"] == "root_cause")["data"]["source_refs"][0]
    assert ref["round"] is None and ref["scope_hash"] is None
    assert any(
        r["kind"] == "coverage" and "scope unavailable" in r["data"]["reason"]
        for r in rows
    )
    scoped = root / "triage.md"
    scoped.write_text("**范围哈希**：" + "1" * 64 + "\n" + scoped.read_text())
    selected = tmp_path / "selected-scope"
    result = collect(root, selected)
    assert result.returncode == 0, result.stderr
    ref = next(r for r in records(selected) if r["kind"] == "root_cause")["data"][
        "source_refs"
    ][0]
    assert ref["round"] == 1 and ref["scope_hash"] == "1" * 64


def test_ready_finder_is_not_its_own_escape_after_gate_corroboration(tmp_path):
    root = tmp_path / "artifacts"
    root.mkdir()
    head = subprocess.check_output(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True
    ).strip()
    review = (FIXTURE / "codexrevf/round-01/review.md").read_text()
    review = (
        review.replace("codexrevf", "finder")
        .replace(
            "fix/codex-rollout-final-answer @ 45fffccda08b626b111afbe61aa77b192c0c9f1d",
            "feature @ " + head,
        )
        .replace("**判定**：Needs Refinement", "**判定**：Ready")
    )
    (root / "review.md").write_text(review)
    (root / "failed.json").write_text(
        json.dumps(
            dict(
                kind="gate",
                head=head,
                result="FAIL",
                started_at="2026-11-02T02:00:00Z",
                finished_at="2026-11-02T02:00:01Z",
                failing_test_ids=["reported-case"],
            )
        )
    )
    prefix = (
        "**模式**：pr\nTarget head "
        + head
        + "\n\n## APPLY\n\n- C1 — src/messaging/model/status.rs:61\n\n## Root cause provenance\n\n| claim_id | outcome | sources |\n|---|---|---|\n"
    )
    triage = root / "triage.md"
    triage.write_text(
        prefix + "| C1 | APPLY | finder@1:1, run@failed.json#reported-case |\n"
    )
    store = tmp_path / "finder-and-gate"
    observed_completion(store, root / "review.md", "finder", "2026-11-02T01:00:00Z")
    result = collect(root, store, ROOT)
    assert result.returncode == 0, result.stderr
    assert not [row for row in records(store) if row["kind"] == "escape"]

    # Without an earlier finding, this exact gate and unchanged blob prove an escape.
    triage.write_text(prefix + "| C1 | APPLY | run@failed.json#reported-case |\n")
    store = tmp_path / "gate-only"
    observed_completion(store, root / "review.md", "finder", "2026-11-02T01:00:00Z")
    result = collect(root, store, ROOT)
    assert result.returncode == 0, result.stderr
    assert len([row for row in records(store) if row["kind"] == "escape"]) == 1
