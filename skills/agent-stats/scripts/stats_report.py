"""Deterministic projections with measured denominators and explicit unknowns."""

import re
from collections import defaultdict
from datetime import datetime
from stats_reviews import review_data, review_key, review_scope

COUNTS = (
    "assignments",
    "authored_prs",
    "handovers",
    "review_rounds",
    "unique_issues",
    "shared_issues",
    "applied",
    "rejected",
    "flagged",
    "withdrawn",
    "escaped_defects",
    "stopped_early",
    "gate_pass",
    "gate_fail",
    "e2e_pass",
    "e2e_fail",
    "live_pass",
    "live_fail",
    "abandoned_deliveries",
)


def projected(rows):
    facts = {}
    for r in rows:
        kind = r["kind"]
        d = r["data"]
        key = r["id"]
        if kind == "workflow":
            key = (r["task"], kind, d["text"])
        elif kind == "pr":
            key = (r["task"], kind, d["number"])
        elif kind == "review":
            key = (kind, *review_key(r))
        elif kind == "run":
            key = (r["task"], kind, r["sources"][0]["sha256"])
        elif kind == "root_cause":
            src = r["sources"][0]
            origin = (
                src["locator"].split("#", 1)[0]
                if "#" in src["locator"]
                else src["path"]
            )
            key = (r["task"], kind, origin, d["claim_id"])
        if (
            key in facts
            and kind == "review"
            and review_data(facts[key]) != review_data(r)
        ):
            raise ValueError("conflicting stored review facts")
        facts[key] = r
    wrapped = {
        sha
        for r in facts.values()
        if r["kind"] == "run"
        for sha in r["data"].get("wrapped_sources", [])
    }
    return [
        r
        for r in facts.values()
        if r["kind"] != "run" or r["sources"][0]["sha256"] not in wrapped
    ]


def report(rows):
    rows = projected(rows)
    board = {}
    reviews = [r for r in rows if r["kind"] == "review"]
    assignments = [r for r in rows if r["kind"] in ("assignment", "handover")]
    last_delivery = {
        (r["task"], r["data"]["request_id"]): r["id"]
        for r in rows
        if r["kind"] == "delivery"
    }
    compactions = {}
    for r in rows:
        if r["kind"] == "compaction":
            compactions[r["agent"]["name"]] = max(
                compactions.get(r["agent"]["name"], 0), r["data"]["count"]
            )

    def entry(row, name=None, actor=None):
        actor = actor or row["agent"]
        name = name or actor["name"]
        model = actor["model"] if name == actor["name"] else "unknown"
        provider = actor["provider"] if name == actor["name"] else "unknown"
        key = (row["role"], provider, model, name)
        if key not in board:
            board[key] = dict(
                role=key[0],
                provider=provider,
                model=model,
                agent=name,
                **{x: 0 for x in COUNTS},
                assignment_outcomes={},
                findings_by_dimension={},
            )
        return board[key]

    timeline = defaultdict(list)
    measured = defaultdict(int)
    unmeasured = set()
    roots = set()
    escaped = set()
    for row in rows:
        kind, data = row["kind"], row["data"]
        timeline[row["task"]].append(row)
        if kind == "coverage":
            unmeasured.add(data["field"] + ": " + data["reason"])
            continue
        if kind == "root_cause":
            key = (row["task"], data["adjudication"], data["claim_id"])
            if key in roots:
                continue
            roots.add(key)
            for lane in data["reviewers"]:
                refs = [r for r in data["source_refs"] if r["reviewer"] == lane]
                related = [
                    r
                    for r in reviews
                    if r["task"] == row["task"]
                    and r["agent"]["name"] == lane
                    and any(
                        x["round"] == r["data"]["round"]
                        and x.get("scope_hash") == review_scope(r)
                        for x in refs
                    )
                ]
                actors = {str(r["agent"]) for r in related}
                value = entry(
                    row, lane, related[0]["agent"] if len(actors) == 1 else None
                )
                value[
                    "unique_issues" if len(data["reviewers"]) == 1 else "shared_issues"
                ] += 1
                value[
                    {
                        "APPLY": "applied",
                        "REJECT": "rejected",
                        "FLAG": "flagged",
                        "WITHDRAWN": "withdrawn",
                    }[data["outcome"]]
                ] += 1
                dimensions = {
                    re.match(r"[1-9]\b", f["dimension"]).group()
                    for r in related
                    for f in r["data"]["findings"]
                    if f["dimension"]
                    and re.match(r"[1-9]\b", f["dimension"])
                    and any(x["finding_id"] == f["id"] for x in refs)
                }
                for dimension in dimensions or {"unknown"}:
                    value["findings_by_dimension"][dimension] = (
                        value["findings_by_dimension"].get(dimension, 0) + 1
                    )
        elif kind == "pr":
            for name in data["authors"]:
                candidates = [
                    r
                    for r in assignments
                    if r["agent"]["name"] == name
                    and r["role"] == "author"
                    and r["task"] in {row["task"], str(data["number"]), data["branch"]}
                    and r["ts"]
                    and datetime.fromisoformat(r["ts"].replace("Z", "+00:00"))
                    <= datetime.fromisoformat(data["merged_at"].replace("Z", "+00:00"))
                ]
                identities = {str(r["agent"]) for r in candidates}
                entry(
                    row, name, candidates[0]["agent"] if len(identities) == 1 else None
                )["authored_prs"] += 1
        elif kind == "review":
            entry(row)["review_rounds"] += 1
        elif kind == "escape":
            key = (row["task"], row["agent"]["name"], data["claim_event"])
            if key not in escaped:
                entry(row)["escaped_defects"] += 1
                escaped.add(key)
        elif kind in ("assignment", "handover"):
            entry(row)["assignments"] += 1
            if kind == "handover":
                entry(row)["handovers"] += 1
        elif kind == "assignment_end":
            value = entry(row)
            value["assignment_outcomes"][data["outcome"]] = (
                value["assignment_outcomes"].get(data["outcome"], 0) + 1
            )
            if data["outcome"] == "stopped_early" and data["proven"]:
                value["stopped_early"] += 1
        elif kind == "run":
            if data["result"] in ("PASS", "FAIL"):
                entry(row)[data["kind"] + "_" + data["result"].lower()] += 1
            if data["duration_ms"] is not None:
                measured[data["kind"] + "_elapsed_ms"] += data["duration_ms"]
            for lane in data["lanes"]:
                if lane["duration_ms"] is not None:
                    measured["lane_" + lane["name"] + "_ms"] += lane["duration_ms"]
            if data["slot_wait_ms"] is not None:
                measured["slot_wait_ms"] += data["slot_wait_ms"]
        elif kind == "phase" and data["transition"] == "end":
            measured["phase_" + data["phase"] + "_ms"] += data["duration_ms"]
        elif (
            kind == "delivery"
            and data["stage"] == "abandoned"
            and last_delivery[(row["task"], data["request_id"])] == row["id"]
        ):
            entry(row)["abandoned_deliveries"] += 1

    def precision(value):
        denominator = value["applied"] + value["rejected"]
        value.update(
            precision_denominator=denominator,
            precision=value["applied"] / denominator if denominator else None,
        )
        return value

    models = {}
    for v in board.values():
        key = (v["role"], v["provider"], v["model"])
        m = models.setdefault(
            key,
            dict(
                role=key[0],
                provider=key[1],
                model=key[2],
                agents=0,
                **{x: 0 for x in COUNTS},
            ),
        )
        m["agents"] = (
            None if v["agent"] == "unknown" or m["agents"] is None else m["agents"] + 1
        )
        for k in COUNTS:
            m[k] += v[k]
    starts = {
        r["id"]
        for r in rows
        if r["kind"] == "phase" and r["data"]["transition"] == "start"
    }
    ends = {
        r["data"]["start_id"]
        for r in rows
        if r["kind"] == "phase" and r["data"]["transition"] == "end"
    }
    return dict(
        leaderboard=sorted(
            [precision(v) for v in board.values()],
            key=lambda x: (x["role"], x["model"], -x["unique_issues"], x["agent"]),
        ),
        model_leaderboard=sorted(
            [precision(v) for v in models.values()],
            key=lambda x: (x["role"], x["model"]),
        ),
        prs={
            k: sorted(v, key=lambda r: (r["ts"] or "", r["kind"], r["id"]))
            for k, v in sorted(timeline.items())
        },
        bottlenecks=dict(
            measured=dict(sorted(measured.items())),
            open_phases=len(starts - ends),
            unmeasured=sorted(unmeasured),
        ),
        root_causes=len(roots),
        rebases_by_task={
            task: len(
                {
                    (r["data"]["base"], r["data"]["old_head"], r["data"]["head"])
                    for r in rows
                    if r["kind"] == "rebase" and r["task"] == task
                }
            )
            for task in sorted({r["task"] for r in rows if r["kind"] == "rebase"})
        },
        compactions_by_agent=dict(sorted(compactions.items())),
    )


def markdown(value):
    lines = [
        "# Agent work statistics",
        "",
        "Observed counts and sample sizes; unknown means unavailable. Workload is not normalized. Proven-event counts do not establish zero incidence when evidence is missing.",
        "",
        "| Role | Provider / model | Agents | Authored PRs | Unique | Shared | APPLY / REJECT | Stopped early |",
        "|---|---|---:|---:|---:|---:|---:|---:|",
    ]
    for r in value["model_leaderboard"]:
        lines.append(
            f"| {r['role']} | {r['provider']} / {r['model']} | {r['agents'] if r['agents'] is not None else 'unknown'} | {r['authored_prs']} | {r['unique_issues']} | {r['shared_issues']} | {r['applied']} / {r['rejected']} | {r['stopped_early']} |"
        )
    lines += [
        "",
        "## Per agent",
        "",
        "| Role | Model | Agent | Assignments | Rounds | Unique | Shared | APPLY / REJECT | Precision | Proven escapes | Proven stopped early |",
        "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for r in value["leaderboard"]:
        p = (
            "unknown"
            if r["precision"] is None
            else f"{r['precision']:.0%} (n={r['precision_denominator']})"
        )
        lines.append(
            f"| {r['role']} | {r['model']} | {r['agent']} | {r['assignments']} | {r['review_rounds']} | {r['unique_issues']} | {r['shared_issues']} | {r['applied']} / {r['rejected']} | {p} | {r['escaped_defects']} | {r['stopped_early']} |"
        )
    lines += [
        "",
        "## Bottlenecks",
        "",
        "Measured lane durations may overlap; they are not additive wall time.",
        "",
    ]
    lines += [
        f"- {name}: {ms / 1000:.3f}s"
        for name, ms in value["bottlenecks"]["measured"].items()
    ]
    lines += [f"- Open task phases: {value['bottlenecks']['open_phases']}"]
    for task, rows in value["prs"].items():
        lines += [
            "",
            "## " + task,
            "",
            "| Time | Fact | Agent | Evidence |",
            "|---|---|---|---|",
        ]
        for r in rows:
            d = r["data"]
            kind = r["kind"]
            if kind == "coverage":
                continue
            detail = kind + ": " + str(d)
            if kind == "pr":
                detail = f"Merged {d['merge_sha'][:8]}; {d['commit_count']} commits; +{d['additions']} / -{d['deletions']}; {d['files']} files; assigned authors: {', '.join(d['authors']) or 'unknown'}"
            elif kind == "review":
                detail = f"R{d['round']} {d['verdict']} @{d['head'][:8]} ({len(d['findings'])} assertions)"
            elif kind == "run":
                detail = f"{d['kind']} {d['result']} @{d['head'][:8]}; elapsed: {str(d['duration_ms']) + 'ms' if d['duration_ms'] is not None else 'unknown'}; cause: {d['failure_cause'] or 'unknown'}"
            elif kind == "root_cause":
                detail = f"root {d['claim_id']} {d['outcome']}: {', '.join(d['reviewers']) or 'runner only'}"
            elif kind == "workflow":
                detail = d["event"] + ": " + d["text"]
            src = r["sources"][0]
            evidence = f"[source]({src['path']})"
            time = r["ts"] or (
                d["date"] + " (day only)" if kind == "review" else "unknown"
            )
            lines.append(
                "| "
                + " | ".join(
                    x.replace("|", "\\|").replace("\n", " ")
                    for x in [time, detail, r["agent"]["name"], evidence]
                )
                + " |"
            )
    lines += ["", "## Coverage omissions", ""] + [
        "- " + x for x in value["bottlenecks"]["unmeasured"]
    ]
    return "\n".join(lines) + "\n"
