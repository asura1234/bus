"""Observe delivery outcomes and cumulative compaction counters from Bus."""

import subprocess
from datetime import datetime, timezone

from stats_identity import envelope
from stats_schema import read_json, record, source
from stats_store import append, read_records, snapshot


def observe(args, store):
    import json

    def read(*argv):
        return json.loads(
            subprocess.run(
                [args.bus, *map(str, argv)], check=True, text=True, capture_output=True
            ).stdout
        )

    rows = read_records(store / "events.jsonl")
    assignments = [r for r in rows if r["kind"] in ("assignment", "handover")]
    if args.task:
        assignments = [r for r in assignments if r["task"] == args.task]
    raw = read_json(args.state) if args.state else read("state")
    state = envelope(raw)
    sp = snapshot(store, raw)
    at = (
        datetime.now(timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )
    result = []
    for aid in sorted({r["agent"]["id"] for r in assignments}):
        row = next(r for r in reversed(assignments) if r["agent"]["id"] == aid)
        matches = [
            a
            for a in state["agents"]
            if str(a["id"]) == aid and a.get("name") == row["agent"]["name"]
        ]
        if len(matches) == 1 and "compactions" in matches[0]:
            data = matches[0]["compactions"]
            result.append(
                record(
                    "compaction",
                    row["task"],
                    row["role"],
                    row["agent"],
                    row["orchestrator"],
                    [source(sp)],
                    dict(count=data["count"], last_at_ms=data["last_at_ms"]),
                    at,
                )
            )
    statuses = [read_json(p) for p in args.status]
    if not args.status:
        statuses = [
            read("message", "status", m)
            for m in sorted(
                {
                    r["data"]["message_id"]
                    for r in assignments
                    if r["data"]["message_id"]
                }
            )
        ]
    for raw in statuses:
        value = envelope(raw)
        saved = snapshot(store, raw)
        matched = [
            r for r in assignments if r["data"]["message_id"] == value["message_id"]
        ]
        if not matched:
            raise ValueError("status has no recorded assignment")
        for request in value["requests"]:
            owner = [r for r in matched if r["agent"]["id"] == str(request["agent_id"])]
            if not owner:
                continue
            row = owner[-1]
            result.append(
                record(
                    "delivery",
                    row["task"],
                    row["role"],
                    row["agent"],
                    row["orchestrator"],
                    [source(saved)],
                    dict(
                        message_id=value["message_id"],
                        request_id=request["request_id"],
                        stage=request["stage"],
                        reason=request.get("reason"),
                    ),
                    at,
                )
            )
    return dict(appended=append(store, result), observations=len(result))
