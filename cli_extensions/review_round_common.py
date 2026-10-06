#!/usr/bin/env python3
"""The shared part of the `/review-pr` and `/review-plan` prologues: lane ownership, round claiming, triage ledger discovery.

Both skills' prologues do the same thing — claim an isolated lane for this review, find its completed
rounds, and fish out the ledgers the author side has already triaged — except one is anchored on the
branch diff and the other on the plan file. This part used to live as one copy each in
`skills/review-pr/scripts/review_round.py` and `skills/review-plan/scripts/review_round_support.py`,
with 10 top-level symbols byte-for-byte identical.

Two copies inevitably drift, and drift here fails silently: a wrong lane ownership raises no error, it
only makes some reviewer read someone else's PREV_REVIEWS, so "cross-round reconciliation" is done
against the wrong history. Wrong ledger discovery lets already-rejected findings reappear, or conversely
suppresses code findings with plan-mode triage.

`skills/AGENTS.md`: Python shared by multiple skills lives in the repo-root `cli_extensions/`, not inside
any one skill directory.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent

TRIAGE_MODE_FIELD_RE = re.compile(
    r"(?im)^\s*(?:\*\*)?(?:review type|review mode|审查类型|模式)(?:\*\*)?\s*[:：]\s*(plan|pr|code)\b"
)
TRIAGE_MODE_TITLE_RE = re.compile(r"(?im)^#.*?[（(]\s*(plan|pr|code)\s*模式")
TRIAGE_MODE_ALIASES = {"plan": "plan", "pr": "pr", "code": "pr"}

# default and default-N are the bare-call family the script claims automatically for "single reviewer /
# concurrent first round" (see claim_bare_round), not named lanes passed explicitly via --reviewer.
DEFAULT_LANE_RE = re.compile(r"default(-\d+)?")


def rel(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def git(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=REPO_ROOT, capture_output=True, text=True)


def triage_mode(path: Path) -> str | None:
    """Read the review mode an author-response ledger explicitly declares; unknown formats fail closed."""
    try:
        content = path.read_text()
    except OSError:
        return None
    preamble = content.split("\n## ", 1)[0]
    match = TRIAGE_MODE_FIELD_RE.search(preamble) or TRIAGE_MODE_TITLE_RE.search(preamble)
    return TRIAGE_MODE_ALIASES.get(match.group(1).lower()) if match else None


def triage_ledgers(branch_slug: str, mode: str) -> list[Path]:
    """All `mode` triage ledgers of this branch, excluding other modes and the task namespace, ordered old→new by mtime.

    triage.md is the product of each address-review run and contains only the findings handled in that
    run — it is **not cumulative**; reading only the newest one misses already-decided items triaged
    earlier or by another lane, and cannot deliver "cross-round / cross-lane suppression". So all
    ledgers are returned, the reviewer reads every one, and for the same issue the newer ledger's
    decision wins.

    `mode` must be passed explicitly rather than inferred: ledgers of the other mode and of task mode
    are not part of this CLOSED WORLD, and suppressing code findings with plan triage (or the reverse)
    silently buries real problems. Sorted by mtime only, independent of directory names / round
    numbers. Returns [] when there are none.
    """
    address_root = REPO_ROOT / "temp" / "address-review-comments"
    triage_root = address_root / branch_slug
    task_namespace = address_root / "__task__"
    if not triage_root.is_dir():
        return []
    candidates = [
        t
        for t in triage_root.glob("*/triage.md")
        if t.is_file() and task_namespace not in t.parents and triage_mode(t) == mode
    ]
    return sorted(candidates, key=lambda p: p.stat().st_mtime)


def lanes_with_history(lane_root: Path) -> list[str]:
    """Names of every lane under this lane root that has a completed round (with review.md), sorted by name.

    Used for the ownership safety check of a bare call (no --reviewer): once a real named lane exists, a
    bare call cannot tell which one to continue, and the user must pass --reviewer explicitly (otherwise
    it would silently fall back to default and lose cross-round ownership).
    """
    if not lane_root.exists():
        return []
    return [
        d.name
        for d in sorted(lane_root.iterdir())
        if d.is_dir() and any(r.is_dir() and (r / "review.md").is_file() for r in d.glob("round-*"))
    ]


def bare_call_ambiguous_lanes(prior_lanes: list[str]) -> list[str]:
    """When a bare call cannot be attributed automatically, return the lanes causing the ambiguity (non-empty means --reviewer is required); return [] when it can continue automatically.

    The **only** solo case that can continue automatically (returns []): no user-named lane exists, and
    the default auto family has at most one completed lane (default interrupted and landing on default-2
    also counts as that one; claim_bare_round continues on it — see its docs).
    Ambiguous (returns every completed lane for the error):
      - a named lane passed explicitly by the user via --reviewer exists (multiple reviewers); or
      - **≥2** completed default-family lanes (several concurrent first rounds each finished default /
        default-2; a bare call cannot tell which to continue, and continuing default would make the
        reviewer that owned default-2 read the wrong PREV_REVIEWS, breaking lane isolation).
    """
    named = [ln for ln in prior_lanes if not DEFAULT_LANE_RE.fullmatch(ln)]
    default_family = [ln for ln in prior_lanes if DEFAULT_LANE_RE.fullmatch(ln)]
    if not named and len(default_family) <= 1:
        return []
    return sorted(named + default_family)


def claim_bare_round(lane_root: Path) -> tuple[str, Path, list[Path]]:
    """Claim a round directory concurrency-safely for a bare call (no --reviewer).

    Prefer continuing the stable 'default' lane (repeated solo bare calls accumulate rounds there); if
    'default' currently has an "in-flight" round (round directory created but no review.md written yet —
    meaning it is held by another concurrent instance, or left over from an interruption), atomically
    skip to the next numbered lane (default-2 / default-3 …). Claiming relies on the atomicity of
    `mkdir(exist_ok=False)`: a given round directory is created by exactly one instance, and the loser of
    the race moves on to the next lane automatically, so concurrent bare calls never overwrite each
    other's traces.

    Returns (reviewer, this round's directory, this lane's completed rounds).

    Cost (known and acceptable): an in-flight round left over from an interruption is no longer reused in
    place; instead the active lane shifts by one (e.g. after default is interrupted, solo follow-up rounds
    land on default-2 automatically and continue there) — no data is lost, only the lane name drifts;
    that is the trade-off for concurrent calls never overwriting each other.
    """
    n = 1
    while True:
        lane = "default" if n == 1 else f"default-{n}"
        lane_dir = lane_root / lane
        rounds = sorted(d for d in lane_dir.glob("round-*") if d.is_dir()) if lane_dir.exists() else []
        completed = [d for d in rounds if (d / "review.md").is_file()]
        inflight = any(not (d / "review.md").is_file() for d in rounds)
        if inflight:
            n += 1
            continue
        target = lane_dir / f"round-{len(completed) + 1:02d}"
        try:
            target.mkdir(parents=True, exist_ok=False)
        except FileExistsError:
            n += 1
            continue
        return lane, target, completed
