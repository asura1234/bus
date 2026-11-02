"""Deterministic read-only collectors for recorded evidence and git objects."""

import re
import subprocess
from pathlib import Path

from stats_artifacts import review, runner, triage, unfenced
from stats_schema import digest, read_json, record, source, unknown


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
    rows, parsed, texts = [], [], []
    orch = unknown()

    def emit(kind, role, data, path, actor=None, ts=None, locator="$"):
        value = record(
            kind,
            task,
            role,
            actor or unknown(),
            orch,
            [source(path, locator)],
            data,
            ts,
        )
        rows.append(value)
        return value

    metadata = read_json(gh) if gh else None
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
        text = path.read_text(encoding="utf-8")
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
            and x["data"]["artifact_sha"] == source(path)["sha256"]
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
    for _, text in texts:
        lanes.update(re.findall(r"\blane=(?:external:)?([\w-]+)\s*\|", text))
    seen_runs = set()
    for path in paths:
        if path.suffix != ".json":
            continue
        value = read_json(path)
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
        if "triage" not in path.name or "**模式**" not in text:
            continue
        token = digest(text)
        if token in seen_triage:
            continue
        seen_triage.add(token)
        items = triage(text, lanes, parsed, run_lookup)
        if not items and re.search(r"(?mi)^## APPLY\s*$", text):
            emit(
                "coverage",
                "reviewer",
                dict(field="root_cause", reason="canonical source mapping unavailable"),
                path,
            )
        for claim, outcome, refs, run_ids, locations in items:
            head_match = re.search(
                r"(?i)(?:head\s*[:=]?\s*`?|@\s*)([0-9a-f]{7,40})\b", text
            )
            head = resolve(repo, head_match.group(1)) if head_match else None
            emit(
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
        for n, text in enumerate(Path(workflow).read_text().splitlines(), 1):
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
