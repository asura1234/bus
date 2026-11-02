"""Deterministic read-only collectors for recorded evidence and git objects."""

import re
import subprocess
from pathlib import Path

from stats_artifacts import (
    review,
    runner,
    triage,
    unfenced,
    markdown_run,
    wrapped_reports,
)
from stats_schema import digest, loads, record, source, unknown


def git(repo, *args):
    return subprocess.run(
        ["git", "-C", str(repo), *args], text=True, capture_output=True, check=True
    ).stdout.strip()


def resolve(repo, sha):
    if repo is None:
        return sha
    try:
        return git(repo, "rev-parse", "--verify", sha + "^{commit}")
    except subprocess.CalledProcessError:
        return sha


def change_stats(repo, old, new):
    lines = git(repo, "diff", "--numstat", old, new).splitlines()
    numbers = [line.split("\t", 2) for line in lines]
    return dict(
        files=len(numbers),
        additions=sum(int(a) for a, _, _ in numbers if a.isdigit()),
        deletions=sum(int(d) for _, d, _ in numbers if d.isdigit()),
    )


def collect(task, artifacts, gh=None, repo=None, workflow=None, history=()):
    root = Path(artifacts)
    if not root.is_dir():
        raise ValueError("invalid artifacts directory")
    import hashlib

    observed = {}

    def read_text_checked(path):
        raw = Path(path).read_bytes()
        key = str(Path(path).resolve())
        checksum = hashlib.sha256(raw).hexdigest()
        if observed.setdefault(key, checksum) != checksum:
            raise ValueError("source changed during collection: " + key)
        return raw.decode("utf-8")

    def json_at(path):
        return loads(read_text_checked(path))

    def checked_source(path, locator="$"):
        proof = source(path, locator)
        if proof["path"] in observed and observed[proof["path"]] != proof["sha256"]:
            raise ValueError("source changed during collection: " + proof["path"])
        return proof

    rows, parsed, texts = [], [], []
    orch = unknown()

    def emit(kind, role, data, path, actor=None, ts=None, locator="$"):
        value = record(
            kind,
            task,
            role,
            actor or unknown(),
            orch,
            [checked_source(path, locator)],
            data,
            ts,
        )
        if kind == "run":
            hashes = {value["sources"][0]["sha256"], *data.get("wrapped_sources", [])}
            bindings = [
                r
                for r in history
                if r["task"] == task
                and r["kind"] == "phase"
                and r["data"]["transition"] == "end"
                and r["data"]["artifact_sha"] in hashes
            ]
            identities = {digest(r["agent"]) for r in bindings}
            if len(identities) > 1:
                raise ValueError("conflicting validation artifact identity bindings")
            if bindings:
                value["agent"] = bindings[0]["agent"]
                value["orchestrator"] = bindings[0]["orchestrator"]
                value["sources"].extend(bindings[0]["sources"])
                value["id"] = digest({k: v for k, v in value.items() if k != "id"})
        rows.append(value)
        return value

    metadata = json_at(gh) if gh else None
    if metadata and (metadata.get("state") != "MERGED" or not metadata.get("mergedAt")):
        raise ValueError("invalid PR metadata: expected merged PR")
    paths = sorted(
        p
        for p in root.rglob("*")
        if p.is_file()
        and not p.is_symlink()
        and p.suffix in (".md", ".json")
        and not any(
            x in p.relative_to(root).parts for x in ("data", "harness", "pinned-source")
        )
    )
    for path in paths:
        if path.suffix != ".md":
            continue
        text = read_text_checked(path)
        plain = unfenced(text)
        texts.append((path, text))
        if plain.startswith("# Review Round ") and "代码审查" in plain.splitlines()[0]:
            lane, data, at = review(text)
            if metadata and data["branch"] != metadata["headRefName"]:
                continue
            data["head"] = resolve(repo, data["head"])
            parsed.append((lane, data, at, path, text))
    unique = {}
    for lane, data, at, path, text in parsed:
        key = (lane, data["round"], data["head"])
        if key in unique:
            if unique[key][1] != data:
                raise ValueError(f"invalid conflicting review copies: {key}")
            continue
        unique[key] = (lane, data, at, path, text)
        bindings = [
            x
            for x in history
            if x["task"] == task
            and x["kind"] == "phase"
            and x["data"]["transition"] == "end"
            and x["data"]["phase"] == "in_review"
            and x["agent"]["name"] == lane
            and x["data"]["artifact_sha"] == checked_source(path)["sha256"]
        ]
        actors = {digest(x["agent"]) for x in bindings}
        if len(actors) > 1:
            raise ValueError("invalid conflicting artifact identity bindings")
        actor = bindings[0]["agent"] if bindings else unknown(lane)
        at = min(x["ts"] for x in bindings) if bindings else at
        emitted = emit(
            "review",
            "reviewer",
            data,
            path,
            actor,
            at,
            "review headers/actionable sections",
        )
        if bindings:
            emitted["orchestrator"] = bindings[0]["orchestrator"]
            emitted["sources"].extend(bindings[0]["sources"])
            emitted["id"] = digest({k: v for k, v in emitted.items() if k != "id"})
    parsed = list(unique.values())
    lanes = {r[0] for r in parsed}
    provenance = {}
    for p, text in texts:
        entries = []
        for lane, round_, ids in re.findall(
            r"\blane=(?:external:)?([\w-]+)(?:[（(][^|]*)?\s*\| round=(\d+) \| file=[^\n]+? \| finding-ids=([^\n]+)",
            text,
        ):
            lanes.add(lane)
            findings = [
                dict(id=x.strip()) for x in ids.split(",") if x.strip().isdigit()
            ]
            entries.append((lane, dict(round=int(round_), findings=findings)))
        if entries:
            provenance[p] = entries
    candidate_reports = []
    for p in paths:
        if p.suffix == ".json":
            value = json_at(p)
            if (
                isinstance(value, dict)
                and {"head", "duration_ms", "exit_code", "results"} <= value.keys()
            ):
                candidate_reports.append((p, value))
    seen_markdown_runs = set()
    for path, text in texts:
        data = markdown_run(text)
        if data and digest(text) not in seen_markdown_runs:
            seen_markdown_runs.add(digest(text))
            linked = (
                wrapped_reports(text, data["head"], candidate_reports)
                if data["kind"] == "gate"
                else []
            )
            if linked:
                data["wrapped_sources"] = [checked_source(p)["sha256"] for p in linked]
                if len(data["lanes"]) == 1:
                    data["duration_ms"] = data["lanes"][0]["duration_ms"]
            emitted = emit("run", "tester", data, path, locator="runner metadata")
            emitted["sources"].extend(
                checked_source(p, "wrapped runner") for p in linked
            )
            emitted["id"] = digest({k: v for k, v in emitted.items() if k != "id"})
    seen_runs = set()
    for path in paths:
        if path.suffix != ".json":
            continue
        value = json_at(path)
        data = runner(value)
        if data is not None:
            token = digest(value)
            if token in seen_runs:
                continue
            seen_runs.add(token)
            emit("run", "tester", data, path, ts=data["started_at"])
    run_lookup = {
        str(Path(row["sources"][0]["path"]).relative_to(root.resolve())): row
        for row in rows
        if row["kind"] == "run"
    }
    seen_triage = set()
    for path, text in texts:
        if (
            "triage" not in path.name
            or "**模式**" not in text
            or any(
                part in ("evidence", "source-reviews", "reviews")
                for part in path.relative_to(root).parts
            )
        ):
            continue
        token = digest(text)
        if token in seen_triage:
            continue
        seen_triage.add(token)
        siblings = [p for p in provenance if p.parent == path.parent]
        inputs = parsed + [entry for p in siblings for entry in provenance[p]]
        items = triage(text, lanes, inputs, run_lookup)
        if not items and re.search(r"(?mi)^## APPLY\s*$", text):
            emit(
                "coverage",
                "reviewer",
                dict(field="root_cause", reason="canonical source mapping unavailable"),
                path,
            )
        for claim, outcome, refs, run_ids, locations in items:
            targets = re.findall(r"(?mi)^(?:\*\*)?Target[^\n]*", text)
            header = "\n".join(targets)
            head_match = re.search(
                r"(?i)(?:head\s*[:=]?\s*`?|@\s*|#\d+\s+)([0-9a-f]{7,40})\b", header
            )
            head = resolve(repo, head_match.group(1)) if head_match else None
            emitted = emit(
                "root_cause",
                "reviewer",
                dict(
                    claim_id=claim,
                    adjudication=token,
                    head=head,
                    outcome=outcome,
                    reviewers=sorted({r["reviewer"] for r in refs}),
                    source_refs=refs,
                    run_ids=run_ids,
                    locations=locations,
                ),
                path,
                locator=f"{outcome}/{claim}",
            )
            for p in siblings:
                emitted["sources"].append(
                    checked_source(p, "sanitized source lane/round/finding ids")
                )
            emitted["id"] = digest({k: v for k, v in emitted.items() if k != "id"})
            if any(r["round"] is None or r["finding_id"] is None for r in refs):
                emit(
                    "coverage",
                    "reviewer",
                    dict(
                        field="root_cause.source_round",
                        reason="exact source assertion or round absent from canonical ledger",
                    ),
                    path,
                )
    if metadata:
        data = dict(
            number=metadata["number"],
            title=metadata["title"],
            url=metadata["url"],
            branch=metadata["headRefName"],
            created_at=metadata["createdAt"],
            merged_at=metadata["mergedAt"],
            merge_sha=metadata["mergeCommit"]["oid"],
            head=metadata["headRefOid"],
            github_author=metadata["author"]["login"],
            commit_count=len(metadata["commits"]),
            commits=[c["oid"] for c in metadata["commits"]],
            additions=metadata["additions"],
            deletions=metadata["deletions"],
            files=metadata["changedFiles"],
            authors=[],
        )
        emit("pr", "author", data, gh, ts=metadata["mergedAt"])
        if repo:
            for row in [
                x
                for x in rows
                if x["kind"] == "review" and x["data"]["verdict"] == "Ready"
            ]:
                try:
                    delta = change_stats(
                        repo, row["data"]["head"], data["merge_sha"] + "^2"
                    )
                except subprocess.CalledProcessError:
                    # Squash merges have one parent; use the published PR head, not
                    # unrelated changes that merged before its squash commit.
                    try:
                        delta = change_stats(repo, row["data"]["head"], data["head"])
                    except subprocess.CalledProcessError:
                        continue
                emit(
                    "delta",
                    "reviewer",
                    dict(
                        ready_head=row["data"]["head"],
                        merged_head=data["head"],
                        **delta,
                    ),
                    gh,
                    row["agent"],
                    locator="$.headRefOid + git diff --numstat",
                )
    for field, reason in [
        ("severity", "review format forbids severity"),
        ("work_start", "no timestamped provisioning completion"),
        ("time_split", "no paired task phase timestamps"),
        ("escaped_defects", "no proven ordered Ready-to-applied-root link"),
        ("failure_cause", "only explicit runner classifications are counted"),
    ]:
        if paths or gh:
            emit(
                "coverage",
                "orchestrator",
                dict(field=field, reason=reason),
                gh or paths[0],
            )
    if workflow:
        for n, text in enumerate(read_text_checked(workflow).splitlines(), 1):
            pr = re.fullmatch(r"pr-(\d+)", task)
            if not pr or not re.search(r"#" + pr.group(1) + r"\b", text):
                continue
            event = (
                "handover"
                if re.search(r"hand(?:ed)?over|replaced", text, re.I)
                else "rebase"
                if "rebase" in text.lower()
                else "compaction"
                if "compaction" in text.lower()
                else "merged"
                if "merged" in text.lower()
                else "ownership"
            )
            emit(
                "workflow",
                "orchestrator",
                dict(text=text, event=event),
                workflow,
                locator=f"L{n}",
            )
    from stats_escapes import escapes

    rows.extend(escapes(rows, repo))
    return rows
