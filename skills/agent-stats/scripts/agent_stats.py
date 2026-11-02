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
from stats_store import append, read_records


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
    data = dict(message_id=args.message)
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
    # Evidence is content addressed, so later input-file changes cannot alter old facts.
    if any(p.is_symlink() for p in [store.absolute(), *store.absolute().parents]):
        raise ValueError("invalid store: symlink path")
    for snapshot, locator in evidence:
        from stats_schema import digest

        saved = store / "raw" / (digest(snapshot) + ".json")
        saved.parent.mkdir(parents=True, exist_ok=True)
        content = canonical(snapshot) + "\n"
        if saved.is_symlink() or (saved.exists() and saved.read_text() != content):
            raise ValueError("invalid evidence snapshot collision")
        if not saved.exists():
            saved.write_text(content)
        paths.append(source(saved, locator))
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
    args = parser.parse_args()
    try:
        result = (
            {"validated": len(read_records(args.file))}
            if args.command == "validate"
            else assignment(args)
        )
        print(canonical(result))
        return 0
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as exc:
        print(f"invalid: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
