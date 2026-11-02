"""Clock-backed task phase boundaries; assignments supply identity, not ratings."""

import time
import subprocess
from datetime import datetime, timezone

from stats_schema import read_json, record, source
from stats_store import append, read_records, snapshot


def phase(args, store):
    rows = read_records(store / "events.jsonl")
    assigned = [
        x
        for x in rows
        if x["task"] == args.task
        and x["kind"] in ("assignment", "handover")
        and (x["agent"]["id"] == args.agent or x["agent"]["name"] == args.agent)
    ]
    if not assigned:
        raise ValueError("invalid task: no assignment for this agent")
    assignment = assigned[-1]
    actor = assignment["agent"]
    previous = [
        x
        for x in rows
        if x["task"] == args.task
        and x["agent"]["id"] == actor["id"]
        and x["kind"] == "phase"
    ]
    open_ = (
        previous[-1]
        if previous and previous[-1]["data"]["transition"] == "start"
        else None
    )
    transition = "start" if args.command == "task-start" else "end"
    if transition == "start" and open_:
        raise ValueError("invalid phase: an earlier phase is still open")
    if transition == "end" and (not open_ or open_["data"]["phase"] != args.phase):
        raise ValueError("invalid phase end: no matching open start")
    now_ns = time.monotonic_ns()
    clock = dict(
        monotonic_ns=now_ns,
        observed_at=datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    )
    try:
        clock["head"] = subprocess.run(
            ["git", "rev-parse", "HEAD"], check=True, text=True, capture_output=True
        ).stdout.strip()
    except subprocess.CalledProcessError:
        clock["head"] = None
    clock["observed_at"] = (
        datetime.now(timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )
    duration = None
    if open_:
        old = read_json(open_["sources"][0]["path"])
        clock["start_monotonic_ns"] = old["monotonic_ns"]
        duration = (now_ns - old["monotonic_ns"]) // 1000000
        if duration < 0:
            raise ValueError(
                "invalid phase clock: host restarted or monotonic clock reset"
            )
    path = snapshot(store, clock)
    sources = [source(path)]
    artifact_sha = None
    if args.artifact:
        if transition != "end":
            raise ValueError("invalid artifact: only completed phases bind artifacts")
        artifact = source(args.artifact)
        artifact_sha = artifact["sha256"]
        sources.append(artifact)
    value = record(
        "phase",
        args.task,
        assignment["role"],
        actor,
        assignment["orchestrator"],
        sources,
        dict(
            phase=args.phase,
            artifact_sha=artifact_sha,
            transition=transition,
            start_id=open_["id"] if open_ else None,
            duration_ms=duration,
        ),
        clock["observed_at"],
    )
    return dict(
        appended=append(store, [value]),
        id=value["id"],
        receipt=str(snapshot(store, value)),
    )


def validate_phases(rows):
    active = {}
    for row in rows:
        if row["kind"] != "phase":
            continue
        key = (row["task"], row["agent"]["id"])
        data = row["data"]
        if data["transition"] == "start":
            if (
                key in active
                or data["start_id"] is not None
                or data["duration_ms"] is not None
            ):
                raise ValueError("invalid duplicate/open phase start")
            active[key] = row
        else:
            start = active.pop(key, None)
            if (
                not start
                or data["start_id"] != start["id"]
                or data["phase"] != start["data"]["phase"]
                or data["duration_ms"] is None
            ):
                raise ValueError("invalid unpaired phase end")
