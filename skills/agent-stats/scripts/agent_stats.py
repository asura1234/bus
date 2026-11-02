#!/usr/bin/env python3
"""Objective work tracking in a repository-local .bus/stats JSONL store."""

import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

from stats_identity import envelope, identity
from stats_schema import SCHEMA, canonical, read_json, record, source
from stats_store import append, read_records, snapshot


def run_json(argv):
    return json.loads(
        subprocess.run(argv, check=True, text=True, capture_output=True).stdout
    )


def default_store():
    common = subprocess.run(
        ["git", "rev-parse", "--git-common-dir"],
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    return Path(common).resolve().parent / ".bus/stats"


def assignment(args):
    store = Path(args.store) if args.store else default_store()
    at = args.at or datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    raw = read_json(args.state) if args.state else run_json([args.bus, "state"])
    state = envelope(raw)
    paths = []
    evidence = [(raw, "$.result.agents")]

    def observed(selector, pane_path):
        a = identity(state, selector)
        if pane_path:
            pane = read_json(pane_path)
        elif not args.state:
            pane = run_json([args.bus, "agent", "read", a["id"], "--source", "visible"])
        else:
            return a
        result = identity(state, selector, envelope(pane))
        evidence.append((pane, "$.result"))
        return result

    actor = observed(args.agent, args.pane)
    orch = observed(args.orchestrator, args.orchestrator_pane)
    a = next(x for x in state["agents"] if str(x["id"]) == actor["id"])
    bound = [
        r
        for r in state["rooms"]
        if r["id"] == a.get("room_id") and str(r.get("orchestrator")) == orch["id"]
    ]
    if not bound:
        raise ValueError("invalid orchestrator: not bound to agent room")
    baseline = None
    if args.workspace_snapshot:
        from stats_outcomes import validate_workspace

        baseline = validate_workspace(read_json(args.workspace_snapshot))
    elif args.worktree:
        from stats_outcomes import workspace

        baseline = workspace(args.worktree)
    workspace_path = snapshot(store, baseline) if baseline is not None else None
    if workspace_path:
        paths.append(source(workspace_path))
    data = dict(
        message_id=args.message,
        room_id=a["room_id"],
        workspace_snapshot=str(workspace_path) if workspace_path else None,
    )
    if args.command == "handover":
        try:
            old = identity(state, args.old_agent)
        except ValueError:
            prior = [
                r
                for r in read_records(store / "events.jsonl")
                if r["kind"] in ("assignment", "handover")
                and r["task"] == args.task
                and r["agent"]["id"] == str(args.old_agent)
            ]
            if not prior:
                raise ValueError("invalid old agent: no recorded assignment") from None
            old = prior[-1]["agent"]
            paths.extend(prior[-1]["sources"])
        data.update(old_agent=old, reason=args.reason)
    for observed_value, locator in evidence:
        paths.append(source(snapshot(store, observed_value), locator))
    value = record(
        args.command if args.command == "handover" else "assignment",
        args.task,
        args.role,
        actor,
        orch,
        paths,
        data,
        at,
    )
    return {"appended": append(store, [value]), "id": value["id"], "store": str(store)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    v = sub.add_parser("validate")
    v.add_argument("file", type=Path)
    for name in ("assign", "handover"):
        p = sub.add_parser(name)
        p.add_argument("--store", type=Path)
        p.add_argument("--task", required=True)
        p.add_argument(
            "--role", choices=SCHEMA["properties"]["role"]["enum"], required=True
        )
        p.add_argument("--agent", required=True)
        p.add_argument("--orchestrator", required=True)
        p.add_argument("--bus", default="bus")
        p.add_argument("--state", type=Path)
        p.add_argument("--pane", type=Path)
        p.add_argument("--orchestrator-pane", type=Path)
        p.add_argument("--worktree", type=Path)
        p.add_argument(
            "--workspace-snapshot",
            type=Path,
            help="replay a recorded git/gh workspace snapshot",
        )
        p.add_argument("--message", type=int)
        p.add_argument("--at")
        if name == "handover":
            p.add_argument("--old-agent", required=True)
            p.add_argument(
                "--reason",
                required=True,
                choices=SCHEMA["data_schemas"]["handover"]["properties"]["reason"][
                    "enum"
                ],
            )
    c = sub.add_parser("collect")
    c.add_argument("--store", type=Path)
    c.add_argument("--task", required=True)
    c.add_argument("--artifacts", type=Path, required=True)
    c.add_argument("--gh", type=Path)
    c.add_argument("--repo", type=Path)
    c.add_argument("--workflow", type=Path)
    r = sub.add_parser("report")
    r.add_argument("--store", type=Path)
    r.add_argument("--json", action="store_true")
    for command in ("task-start", "task-end"):
        p = sub.add_parser(command)
        p.add_argument("--store", type=Path)
        p.add_argument("--task", required=True)
        p.add_argument("--agent", required=True)
        p.add_argument(
            "--phase",
            choices=SCHEMA["data_schemas"]["phase"]["properties"]["phase"]["enum"],
            required=True,
        )
        p.add_argument("--artifact", type=Path)
    e = sub.add_parser("assignment-end")
    e.add_argument("--store", type=Path)
    e.add_argument("--assignment", required=True)
    e.add_argument(
        "--outcome",
        choices=SCHEMA["data_schemas"]["assignment_end"]["properties"]["outcome"][
            "enum"
        ],
        required=True,
    )
    e.add_argument("--bus", default="bus")
    e.add_argument(
        "--evidence",
        type=Path,
        help="replay recorded Bus status/history and git/gh evidence",
    )
    args = parser.parse_args()
    try:
        if args.command == "validate":
            result = {"validated": len(read_records(args.file))}
        elif args.command in ("task-start", "task-end"):
            from stats_phases import phase

            result = phase(args, args.store or default_store())
        elif args.command == "assignment-end":
            from stats_outcomes import end_assignment

            result = end_assignment(args, args.store or default_store())
        elif args.command == "collect":
            from stats_collect import collect

            store = args.store or default_store()
            records = collect(
                args.task,
                args.artifacts,
                args.gh,
                args.repo,
                args.workflow,
                read_records(store / "events.jsonl"),
            )
            from stats_evidence import preserve

            records = preserve(records, store)
            result = {"appended": append(store, records), "collected": len(records)}
        elif args.command == "report":
            from stats_report import report, markdown

            result = report(
                read_records((args.store or default_store()) / "events.jsonl")
            )
            if not args.json:
                print(markdown(result), end="")
                return 0
        else:
            result = assignment(args)
        print(canonical(result))
        return 0
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as exc:
        print(f"invalid: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
