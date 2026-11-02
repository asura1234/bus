"""Fetch PR facts and replay recorded artifacts into the local evidence store."""

import json
import re
import subprocess
from pathlib import Path

from stats_collect import collect
from stats_evidence import preserve
from stats_schema import read_json
from stats_store import append, read_records, snapshot

FIELDS = "number,title,url,state,headRefName,headRefOid,createdAt,mergedAt,mergeCommit,commits,additions,deletions,changedFiles,author"


def backfill(args, store):
    tokens = args.prs.split(",")
    if not all(re.fullmatch(r"[1-9][0-9]*", x) for x in tokens) or len(
        set(tokens)
    ) != len(tokens):
        raise ValueError("PR list must contain distinct positive numbers")
    summaries = []
    for token in tokens:
        if args.gh_dir:
            metadata = read_json(args.gh_dir / (token + ".json"))
        else:
            metadata = json.loads(
                subprocess.run(
                    ["gh", "pr", "view", token, "--json", FIELDS],
                    cwd=args.repo,
                    check=True,
                    text=True,
                    capture_output=True,
                ).stdout
            )
        if metadata["number"] != int(token):
            raise ValueError("PR metadata number mismatch")
        gh = snapshot(store, metadata)
        rows = collect(
            "pr-" + token,
            Path(args.artifacts_root) / ("pr-" + token),
            gh,
            args.repo,
            args.workflow,
            read_records(store / "events.jsonl"),
        )
        rows = preserve(rows, store)
        summaries.append(
            dict(pr=int(token), collected=len(rows), appended=append(store, rows))
        )
    saved = snapshot(
        store, dict(prs=summaries, metadata_sources="gh pr view --json " + FIELDS)
    )
    return dict(
        appended=sum(x["appended"] for x in summaries),
        prs=summaries,
        manifest=str(saved),
    )
