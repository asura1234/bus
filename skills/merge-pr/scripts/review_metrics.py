#!/usr/bin/env python3
"""Append one line of review metrics per merged PR, for judging long-term whether the machine review path is worth running.

A single triage ledger stays in `temp/` and disappears with that run; "is this process saving money" can only be answered by **cross-PR
trends**, so it needs a landing place that does not disappear with the run.

Everything mechanically available is taken here (review rounds, thread count, resolved count, verdict at the time, diff size); only the adjudication counts
must be passed in by the caller — APPLY / REJECT / FLAG are the main agent's judgment results, and GitHub has no such fact.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
# Lands under `temp/`: it is an ignored local artifact. This is not for convenience — the merge happens after the PR has landed,
# when no branch can carry a new commit (this repo forbids committing to master directly), and writing an **untracked** in-repo
# file would immediately fail the next merge-pr / gate-and-fix precondition "the worktree must be clean".
# The cost is that it exists only on the machine that ran the merge: fresh clones and CI runners cannot read the history. This ledger's purpose is exactly
# "the developer watching trends long-term on their own machine", so the cost fits the purpose; aggregating across machines needs a landing place outside the repo.
DEFAULT_LEDGER = "temp/review-metrics.jsonl"
# Same pattern as the lane: finding sections carried in the body itself.
BODY_FINDINGS_RE = re.compile(r"(?:suppressed\s+comments|previously\s+missed|open)\s*\((?!0\))\d+\)", re.IGNORECASE)


def _gh_json(*args: str, cwd: Path | None = None):
    """`gh` must run inside **the selected repository**.

    Without `cwd` it resolves `{owner}/{repo}` from the caller's current directory, so running `--repo <path>` from another
    checkout queries repository A's PR and writes the record into repository B's ledger — a line describing the wrong PR's data,
    while this ledger's entire purpose is long-term trends.
    """

    done = subprocess.run(["gh", *args], capture_output=True, text=True, check=False, cwd=str(cwd) if cwd else None)
    if done.returncode != 0:
        raise RuntimeError(f"gh {' '.join(args)} failed: {done.stderr.strip()}")
    return json.loads(done.stdout)


def collect(pr: str, repo: Path) -> dict:
    """The part mechanically available from GitHub and git."""

    reviews = _gh_json("api", f"repos/{{owner}}/{{repo}}/pulls/{pr}/reviews?per_page=100", "--paginate", cwd=repo)
    threads = _gh_json("api", f"repos/{{owner}}/{{repo}}/pulls/{pr}/comments?per_page=100", "--paginate", cwd=repo)
    info = _gh_json(
        "pr", "view", pr, "--json", "baseRefName,headRefOid,additions,deletions,changedFiles,mergedAt", cwd=repo
    )

    # One review pass = a review with a body, **or** a state-only `APPROVED` / `CHANGES_REQUESTED`.
    # Filtering only on a nonempty body would record a clear conclusion like "a human only clicked Request changes" as 0 passes.
    def is_pass(r: dict) -> bool:
        state = (r.get("state") or "").upper()
        if state in {"APPROVED", "CHANGES_REQUESTED"}:
            return True
        return bool((r.get("body") or "").strip())

    bodied = [r for r in reviews if is_pass(r)]
    # Count only **originating** inline findings, not replies: replies hang off `in_reply_to_id`.
    findings = [c for c in threads if c.get("in_reply_to_id") is None]
    replies = {c["in_reply_to_id"] for c in threads if c.get("in_reply_to_id")}
    # Findings carried in the body (`Open (N)` / `Suppressed comments (N)` / `Previously missed (N)`) have no
    # corresponding inline comment; counting only inline would record a whole body-only review round as "ran once, 0 findings",
    # erasing exactly the kind of work this change newly covers. Report the two separately, not merged into one "total".
    body_findings = sum(bool(BODY_FINDINGS_RE.search(r.get("body") or "")) for r in reviews)

    return {
        "pr": int(pr),
        "mergedAt": info.get("mergedAt") or datetime.now(timezone.utc).isoformat(),
        "base": info.get("baseRefName"),
        "head": info.get("headRefOid", "")[:12],
        "files": info.get("changedFiles"),
        "insertions": info.get("additions"),
        "deletions": info.get("deletions"),
        # How many passes review ran. High = slow convergence = this process is not saving money on this PR.
        "reviewPasses": len(bodied),
        # Deduplicated finding total, and how many of them we replied to and how many it marked resolved.
        "inlineFindings": len(findings),
        "reviewsWithBodyFindings": body_findings,
        "repliedFindings": len(replies),
    }


def append(record: dict, ledger: Path) -> Path:
    ledger.parent.mkdir(parents=True, exist_ok=True)
    with ledger.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, ensure_ascii=False, sort_keys=True) + "\n")
    return ledger


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pr", required=True)
    parser.add_argument("--repo", default=str(REPO_ROOT))
    parser.add_argument("--ledger", default=None)
    # Adjudication counts have no machine source of truth and must be passed in truthfully by the main agent.
    parser.add_argument("--applied", type=int, required=True)
    parser.add_argument("--rejected", type=int, required=True)
    parser.add_argument("--flagged", type=int, default=0)
    parser.add_argument("--verdict", default=None, help="the verdict text at merge time, e.g. \"🔵 Needs a closer look\"")
    args = parser.parse_args(argv)

    repo = Path(args.repo)
    ledger = Path(args.ledger) if args.ledger else repo / DEFAULT_LEDGER
    try:
        record = collect(args.pr, repo)
    except (RuntimeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    record.update(
        {"applied": args.applied, "rejected": args.rejected, "flagged": args.flagged, "verdictAtMerge": args.verdict}
    )
    print(json.dumps({"ledger": str(append(record, ledger)), "record": record}, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
