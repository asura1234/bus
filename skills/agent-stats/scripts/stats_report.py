"""Deterministic summaries of observed facts, with explicit denominators."""

from collections import defaultdict


def report(rows):
    board = {}

    def entry(row, name=None):
        actor = row["agent"]
        model = actor["model"] if name is None or name == actor["name"] else "unknown"
        name = name or actor["name"]
        key = (row["role"], model, name)
        if key not in board:
            board[key] = dict(
                role=key[0],
                model=model,
                agent=name,
                assignments=0,
                handovers=0,
                review_rounds=0,
                escaped_defects=0,
                unique_issues=0,
                shared_issues=0,
                applied=0,
                rejected=0,
                flagged=0,
                withdrawn=0,
                gate_pass=0,
                gate_fail=0,
                e2e_pass=0,
                e2e_fail=0,
                live_pass=0,
                live_fail=0,
                abandoned_deliveries=0,
            )
        return board[key]

    timeline = defaultdict(list)
    measured = defaultdict(int)
    unmeasured = set()
    roots = set()
    for row in sorted(
        rows, key=lambda x: (x["task"], x["ts"] or "", x["kind"], x["id"])
    ):
        kind, data = row["kind"], row["data"]
        timeline[row["task"]].append(row)
        if kind == "coverage":
            unmeasured.add(data["field"] + ": " + data["reason"])
            continue
        if kind == "root_cause":
            root = (row["task"], data["adjudication"], data["claim_id"])
            if root in roots:
                continue
            roots.add(root)
            for lane in data["reviewers"]:
                value = entry(row, lane)
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
        elif kind == "escape":
            entry(row)["escaped_defects"] += 1
        elif kind == "review":
            entry(row)["review_rounds"] += 1
        elif kind in ("assignment", "handover"):
            entry(row)["assignments"] += 1
            if kind == "handover":
                entry(row)["handovers"] += 1
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
        elif kind == "delivery" and data["stage"] == "abandoned":
            entry(row)["abandoned_deliveries"] += 1
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
    for value in board.values():
        denominator = value["applied"] + value["rejected"]
        value["precision_denominator"] = denominator
        value["precision"] = value["applied"] / denominator if denominator else None
    return dict(
        leaderboard=sorted(
            board.values(),
            key=lambda x: (x["role"], x["model"], -x["unique_issues"], x["agent"]),
        ),
        prs=dict(sorted(timeline.items())),
        bottlenecks=dict(
            measured=dict(sorted(measured.items())),
            open_phases=len(starts - ends),
            unmeasured=sorted(unmeasured),
        ),
        root_causes=len(roots),
    )


def markdown(value):
    lines = [
        "# Agent work statistics",
        "",
        "Observed counts only; unknown means unavailable. Different workloads are not normalized.",
        "",
        "| Role | Model | Agent | Assignments | Review rounds | Unique | Shared | APPLY | REJECT | Precision |",
        "|---|---|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for row in value["leaderboard"]:
        precision = (
            "unknown"
            if row["precision"] is None
            else f"{row['precision']:.0%} ({row['precision_denominator']})"
        )
        lines.append(
            "| "
            + " | ".join(
                str(row[x]).replace("|", "\\|")
                for x in (
                    "role",
                    "model",
                    "agent",
                    "assignments",
                    "review_rounds",
                    "unique_issues",
                    "shared_issues",
                    "applied",
                    "rejected",
                )
            )
            + " | "
            + precision
            + " |"
        )
    lines += [
        "",
        "## Bottlenecks",
        "",
        "Measured elapsed and lane durations can overlap; they are not additive wall time.",
        "",
    ]
    for name, ms in value["bottlenecks"]["measured"].items():
        lines.append(f"- {name}: {ms / 1000:.3f}s")
    for task, rows in value["prs"].items():
        lines += [
            "",
            "## " + task,
            "",
            "| Time | Fact | Agent | Evidence |",
            "|---|---|---|---|",
        ]
        for row in rows:
            data = row["data"]
            if row["kind"] == "coverage":
                continue
            detail = row["kind"] + ": " + str(data)
            if row["kind"] == "review":
                detail = f"R{data['round']} {data['verdict']} @{data['head'][:8]} ({len(data['findings'])} assertions)"
            elif row["kind"] == "run":
                detail = f"{data['kind']} {data['result']} @{data['head'][:8]}, duration={data['duration_ms']}ms, cause={data['failure_cause']}"
            elif row["kind"] == "root_cause":
                detail = f"root {data['claim_id']} {data['outcome']}: {', '.join(data['reviewers'])}"
            src = row["sources"][0]
            evidence = f"[{src['locator']}]({src['path']})"
            lines.append(
                "| "
                + " | ".join(
                    x.replace("|", "\\|").replace("\n", " ")
                    for x in [
                        row["ts"] or "unknown",
                        detail,
                        row["agent"]["name"],
                        evidence,
                    ]
                )
                + " |"
            )
    lines += ["", "## Coverage omissions", ""] + [
        "- " + x for x in value["bottlenecks"]["unmeasured"]
    ]
    return "\n".join(lines) + "\n"
