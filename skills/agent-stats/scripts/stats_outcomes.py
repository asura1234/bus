"""Record assignment ends; prove early stopping from bounded machine evidence."""

import subprocess
from datetime import datetime, timezone

from stats_identity import envelope
from stats_schema import read_json, read_source, record, source
from stats_store import append, read_records, snapshot


def validate_workspace(value):
    import re

    if (
        not isinstance(value, dict)
        or set(value) != {"repo", "branch", "head", "prs"}
        or not isinstance(value["repo"], str)
        or not value["repo"].startswith("/")
        or not isinstance(value["branch"], str)
        or not value["branch"]
        or not re.fullmatch(r"[0-9a-f]{40,64}", value["head"])
        or not isinstance(value["prs"], list)
    ):
        raise ValueError("invalid git/gh workspace snapshot")
    for pr in value["prs"]:
        if (
            set(pr) != {"number", "headRefOid", "state"}
            or type(pr["number"]) is not int
            or pr["number"] < 1
            or pr["state"] not in ("OPEN", "CLOSED", "MERGED")
            or not re.fullmatch(r"[0-9a-f]{40,64}", pr["headRefOid"])
        ):
            raise ValueError("invalid PR workspace snapshot")
    return value


def workspace(repo):
    def git(*args):
        return subprocess.run(
            ["git", "-C", str(repo), *args], check=True, text=True, capture_output=True
        ).stdout.strip()

    root = git("rev-parse", "--show-toplevel")
    branch = git("branch", "--show-current")
    if not branch:
        raise ValueError("task workspace must have a branch")
    import json

    prs = json.loads(
        subprocess.run(
            [
                "gh",
                "pr",
                "list",
                "--state",
                "all",
                "--head",
                branch,
                "--json",
                "number,headRefOid,state",
            ],
            cwd=root,
            check=True,
            text=True,
            capture_output=True,
        ).stdout
    )
    return dict(
        repo=root,
        branch=branch,
        head=git("rev-parse", "HEAD"),
        prs=sorted(prs, key=lambda x: x["number"]),
    )


def capture(assignment, bus, baseline):
    import json

    def read(*args):
        return json.loads(
            subprocess.run(
                [bus, *map(str, args)], check=True, text=True, capture_output=True
            ).stdout
        )

    data = assignment["data"]
    state = read("state")
    status = (
        read("message", "status", data["message_id"]) if data["message_id"] else None
    )
    pages = []
    before = None
    while data["message_id"]:
        args = ["history", "--room", data["room_id"], "--limit", 100]
        if before is not None:
            args += ["--before", before]
        page = read(*args)
        value = envelope(page)
        pages.append(page)
        ids = [m["prompt"]["id"] for m in value["messages"]]
        if not value.get("has_more") or (ids and min(ids) <= data["message_id"]):
            break
        next_ = value.get("next_before")
        if not next_ or next_ == before:
            raise ValueError("history pagination did not advance")
        before = next_
    return dict(
        state=state,
        status=status,
        history_pages=pages,
        workspace=workspace(baseline["repo"]) if baseline else None,
    )


def prove(assignment, rows, bundle, baseline):
    data = assignment["data"]
    actor = assignment["agent"]["id"]
    retries = [
        r
        for r in rows
        if r["task"] == assignment["task"]
        and r["id"] != assignment["id"]
        and (
            (
                r["kind"] == "assignment"
                and r["agent"]["id"] == actor
                and r["data"]["message_id"]
                and data["message_id"]
                and r["data"]["message_id"] > data["message_id"]
            )
            or (
                r["kind"] == "handover"
                and r["data"]["old_agent"]["id"] == actor
                and r["data"]["reason"] == "stopped"
            )
        )
    ]
    ids = sorted(
        {
            x
            for x in [data["message_id"], *[r["data"]["message_id"] for r in retries]]
            if x
        }
    )
    if not retries or not data["message_id"]:
        return False, "no recorded retry or stopped handover", ids
    agents = envelope(bundle["state"])["agents"]
    if not any(str(a["id"]) == actor and a.get("status") == "idle" for a in agents):
        return False, "no observed idle state", ids
    if bundle.get("status") is None:
        return False, "no assignment request status", ids
    status = envelope(bundle["status"])
    requests = [r for r in status["requests"] if str(r["agent_id"]) == actor]
    if (
        status["message_id"] != data["message_id"]
        or len(requests) != 1
        or requests[0].get("turn_ended") is not True
        or requests[0].get("reply") is not None
    ):
        return False, "request did not end without a captured result", ids
    pages = [envelope(p) for p in bundle.get("history_pages", [])]
    if not pages or any(p["room_id"] != data["room_id"] for p in pages):
        return False, "missing home-room history", ids
    oldest = pages[-1]
    message_ids = [m["prompt"]["id"] for p in pages for m in p["messages"]]
    if oldest.get("has_more") is not False and (
        not message_ids or min(message_ids) > data["message_id"]
    ):
        return False, "history does not cover the assignment interval", ids
    for page in pages:
        for msg in page["messages"]:
            prompt = msg["prompt"]
            if (
                prompt["id"] > data["message_id"]
                and str(prompt.get("author", {}).get("agent")) == actor
            ):
                return (
                    False,
                    "agent sent a message after assignment; no-result absence is unproven",
                    ids,
                )
    if baseline:
        validate_workspace(baseline)
    if bundle.get("workspace") is not None:
        validate_workspace(bundle["workspace"])
    if not baseline or bundle.get("workspace") != baseline:
        return False, "tracked task branch or PR set changed or is unmeasured", ids
    return (
        True,
        "idle ended request; covered home-room history has no authored result; tracked task branch and PR set unchanged",
        ids,
    )


def end_assignment(args, store):
    rows = read_records(store / "events.jsonl")
    matches = [
        r
        for r in rows
        if r["id"] == args.assignment and r["kind"] in ("assignment", "handover")
    ]
    if len(matches) != 1:
        raise ValueError("unknown assignment id")
    assignment = matches[0]
    baseline = None
    if assignment["data"]["workspace_snapshot"]:
        proofs = [
            s
            for s in assignment["sources"]
            if s["path"] == assignment["data"]["workspace_snapshot"]
        ]
        if len(proofs) != 1:
            raise ValueError("missing task workspace source")
        baseline = read_source(proofs[0])
    bundle = (
        read_json(args.evidence)
        if args.evidence
        else capture(assignment, args.bus, baseline)
    )
    proven, reason, ids = (
        prove(assignment, rows, bundle, baseline)
        if args.outcome == "stopped_early"
        else (
            False,
            "orchestrator-recorded assignment outcome",
            [assignment["data"]["message_id"]]
            if assignment["data"]["message_id"]
            else [],
        )
    )
    saved = snapshot(store, bundle)
    value = record(
        "assignment_end",
        assignment["task"],
        assignment["role"],
        assignment["agent"],
        assignment["orchestrator"],
        assignment["sources"] + [source(saved)],
        dict(
            assignment_id=assignment["id"],
            outcome=args.outcome,
            proven=proven,
            reason=reason,
            message_ids=ids,
        ),
        datetime.now(timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z"),
    )
    return dict(
        appended=append(store, [value]), id=value["id"], proven=proven, reason=reason
    )
