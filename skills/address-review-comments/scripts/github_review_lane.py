#!/usr/bin/env python3
"""The **signal-fetching** and **posting-back** ends for GitHub bot review comments; the transcription in between is
not here.

This script only does the mechanical, fail-closed parts: pull **two** signal streams (inline comments and review
bodies), split them into rounds, write the latest round's raw text together with timestamps to disk, and post the
verdict ledger back to the PR. **Transcribing the raw text into review.md is the agent's job**, written in SKILL.md
-- transcription requires understanding what each comment is saying, which dimension it falls into, and what the
evidence is; that is judgment, not string concatenation.

Round identity is taken from the commit the comments are pinned to, with timestamps as a backstop for the
"re-reviewed on the same commit" case: the anchor is the same but the comments are newer, and judging by the anchor
alone would misjudge it as "this round has already been fetched".

Subcommands:
  fetch       --pr N   fetch the latest round's raw comments + timestamps into temp/review-pr/<slug>/github/round-NN/
  post-triage --pr N   post the triage ledger back to the PR verbatim
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

from review_lane_core import TRIAGED_MARKER, LaneError, _gh, base_revision, branch_slug, local_branch, safe_lane
from review_lane_github import fetch_rounds, head_verdict, post_triage, pr_view, reply_to_finding, review_coverage
from review_lane_rounds import (
    classify_review,
    content_digest,
    existing_rounds,
    item_digests,
    item_key,
    latest_anchor,
    render_source,
    review_headings,
    review_headline,
    review_is_clean,
    round_identity,
    round_is_triaged,
    write_triaged_marker,
)


# Tests and callers reference and monkeypatch via `github_review_lane.X` (`_gh`, `local_branch`, `subprocess`, etc.).
__all__ = [
    "TRIAGED_MARKER",
    "LaneError",
    "_gh",
    "base_revision",
    "branch_slug",
    "classify_review",
    "content_digest",
    "fetch",
    "head_verdict",
    "local_branch",
    "main",
    "post_triage",
    "reply_to_finding",
    "review_coverage",
    "review_headings",
    "review_headline",
    "review_is_clean",
    "round_identity",
    "round_is_triaged",
    "subprocess",
    "write_triaged_marker",
]


REPO_ROOT = Path(__file__).resolve().parents[3]


def fetch(pr: str, repo: Path, lane: str) -> dict:
    """Write the latest round's raw comments to disk, and tell the caller whether this round is new.

    Idempotency is judged by **content**, not by anchor: the merge-pr loop calls in every round, and a bot review can
    rerun on the same commit, in which case the anchor is unchanged but the comments are new. Comparing only the
    anchor would treat a new round as already fetched.
    """

    before = pr_view(pr)
    rounds = fetch_rounds(pr)
    # Re-read HEAD after fetching the comments. If someone pushes between the two requests, the comments come from
    # the new state while head is a stale snapshot, and `anchor == head` may hold as a result -- the lane would
    # claim to cover the current HEAD, then go on adjudicating and landing on **a tree no review has looked at**.
    # When the two disagree, don't guess; have the caller refetch.
    info = pr_view(pr)
    if info["headRefOid"] != before["headRefOid"]:
        return {
            "pr": info["number"],
            "status": "head-moved",
            "lane": lane,
            "head": info["headRefOid"],
            "previousHead": before["headRefOid"],
            "roundCount": len(rounds),
        }

    # The slug must use the **locally checked-out branch**, not the PR's headRefName. The artifact is for
    # address-review-comments' lane detection, and that side (review_round.py) builds directories by
    # `git branch --show-current`. With a fork checkout or a locally renamed branch the two differ, and
    # writing into the headRefName directory amounts to writing somewhere nobody will glob.
    slug = branch_slug(local_branch(repo))
    lane_dir = repo / "temp" / "review-pr" / slug / safe_lane(lane)

    # `no-comments` = this stream has **no pending artifacts at all**: neither inline comments nor pending review
    # bodies. Bodies with a clean verdict were already filtered out in fetch_review_bodies and won't prop up an
    # empty round.
    if not rounds:
        return {
            "pr": info["number"],
            "status": "no-comments",
            "lane": lane,
            "head": info["headRefOid"],
            "roundCount": 0,
        }

    # A review can give a clean re-review on the current HEAD **without any inline comments**. The inline comment
    # group then still sits on the old anchor, and looking only at `rounds` would conclude "the review hasn't seen
    # this tree" and STOP -- when it clearly has, it just had no remarks. Without this, a clean re-review could
    # never move merge-pr forward.
    # No need to judge the verdict again here: **a pending body is already an anchor group**, so as long as the
    # current HEAD has a 🟡/🔵/unrecognized review, HEAD is in `rounds`, and the `not any(...)` below is false on
    # its own. The only case that can reach clean is "the reviews on HEAD are all clean verdicts with no remarks
    # section".
    clean_at_head = not any(a == info["headRefOid"] for a in rounds) and review_coverage(pr)["headHasReview"]

    anchor = latest_anchor(rounds)
    comments = rounds[anchor]["comments"]
    reviews = rounds[anchor]["reviews"]
    identity = round_identity(comments, reviews)

    prior = existing_rounds(lane_dir)
    for number, round_dir in prior:
        recorded = json.loads((round_dir / "source.json").read_text(encoding="utf-8"))
        if recorded.get("anchor") == anchor and recorded.get("contentDigest") == identity["contentDigest"]:
            return {
                "pr": info["number"],
                "status": "reused",
                "lane": lane,
                "round": number,
                "anchor": anchor,
                "head": info["headRefOid"],
                "roundIsCurrentHead": anchor == info["headRefOid"],
                "cleanReviewAtHead": clean_at_head,
                "reviewExists": (round_dir / "review.md").is_file(),
                "triaged": round_is_triaged(round_dir),
                "source": str(round_dir / "source.md"),
                "roundDir": str(round_dir),
                **identity,
            }

    # Render only the **delta relative to rounds already on disk**. When a review reruns on the same commit, old and
    # new comments merge into the same anchor group; rendering the whole group would re-disposition and re-post
    # every previously adjudicated item -- the exact opposite of "only disposition the latest round".
    # `seen` accumulates only from **fully dispositioned** rounds. A round existing doesn't mean it finished:
    # transcription happens before verification, adjudication and landing, and STOPping midway on a FLAG or
    # crashing both leave a round directory behind. If such half-finished rounds also counted as consumed, then
    # when new comments arrive on the same anchor, the remarks in the half-finished round would be subtracted from
    # the delta -- never adjudicated, and never again appearing in any pending round, effectively vanishing.
    seen_digests: dict[str, str] = {}
    seen_ids: set[str] = set()
    legacy_rounds: list[tuple[set[str], str | None]] = []
    consumed: tuple[int, Path] | None = None
    for number, round_dir in prior:
        if not round_is_triaged(round_dir):
            continue
        recorded = json.loads((round_dir / "source.json").read_text(encoding="utf-8"))
        if recorded.get("anchor") != anchor:
            continue
        consumed = (number, round_dir)
        recorded_digests = recorded.get("itemDigests")
        if recorded_digests:
            seen_digests.update(recorded_digests)
        else:
            # Rounds from before this field only recorded ids. Keep recognizing them by id, with semantics verbatim
            # identical to before the upgrade -- otherwise, the moment the upgrade lands, every dispositioned item in
            # every round on disk would be treated as new because "no digest can be found".
            ids = {f"c{cid}" for cid in recorded.get("commentIds") or []}
            ids |= {f"r{rid}" for rid in recorded.get("reviewIds") or []}
            seen_ids.update(ids)
            # Each legacy round remembers **its own** batch of ids and its own digest. If they were merged into one
            # before comparing, then whenever the same anchor has had two rounds (round 1 id 1, round 2 adds id 2),
            # the union digest would never match the last round's digest, so every upgrade would reopen the
            # dispositioned remarks wholesale.
            legacy_rounds.append((ids, recorded.get("contentDigest")))

    # Legacy rounds only recorded ids, so "same id but edited content" can't be told apart there: recognizing by id
    # would treat a rewritten remark as dispositioned, and it would then never appear in any round again. The
    # approach is to recompute the whole-group digest from the current content of **that original batch of ids**
    # and compare it with the one the legacy round recorded -- if different, something in that batch was edited,
    # and the whole group is reopened.
    # We can't compare the whole-round digest directly: adding a comment changes it too, which would turn every
    # addition into a whole-group replay.
    for ids, digest in legacy_rounds:
        legacy_comments = [c for c in comments if item_key("c", c) in ids]
        legacy_reviews = [r for r in reviews if item_key("r", r) in ids]
        if content_digest(legacy_comments, legacy_reviews) != digest:
            seen_ids -= ids

    current = item_digests(comments, reviews)

    def is_fresh(kind: str, item: dict) -> bool:
        key = item_key(kind, item)
        return key not in seen_ids and seen_digests.get(key) != current[key]

    fresh = [c for c in comments if is_fresh("c", c)]
    fresh_reviews = [r for r in reviews if is_fresh("r", r)]

    # If neither stream has anything new, every item on this anchor has already been dispositioned, and the only
    # possible reason the digest differs is that **something was removed** (a body withdrawn, a comment deleted).
    # In that case we **must not** fall back to reopening a whole-group round: that would re-disposition, as-is,
    # items already adjudicated and already posted in a ledger. The correct answer is "this round is done", i.e.
    # reuse that dispositioned round.
    if not fresh and not fresh_reviews and consumed is not None:
        number, round_dir = consumed
        return {
            "pr": info["number"],
            "status": "reused",
            "lane": lane,
            "round": number,
            "anchor": anchor,
            "head": info["headRefOid"],
            "roundIsCurrentHead": anchor == info["headRefOid"],
            "cleanReviewAtHead": clean_at_head,
            "reviewExists": (round_dir / "review.md").is_file(),
            "triaged": round_is_triaged(round_dir),
            "source": str(round_dir / "source.md"),
            "roundDir": str(round_dir),
            **identity,
        }

    base_ref, base_sha = base_revision(repo, info["baseRefName"])
    number = (prior[-1][0] + 1) if prior else 1
    round_dir = lane_dir / f"round-{number:02d}"
    round_dir.mkdir(parents=True, exist_ok=True)
    (round_dir / "source.md").write_text(render_source(info, anchor, fresh, fresh_reviews), encoding="utf-8")
    (round_dir / "source.json").write_text(
        json.dumps(
            {
                "pr": info["number"],
                "anchor": anchor,
                "branch": info["headRefName"],
                "head": info["headRefOid"],
                "base": info["baseRefName"],
                "baseRef": base_ref,
                "baseSha": base_sha,
                **round_identity(fresh, fresh_reviews),
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    return {
        "pr": info["number"],
        "status": "created",
        "lane": lane,
        "round": number,
        "anchor": anchor,
        "head": info["headRefOid"],
        "baseRef": base_ref,
        "baseSha": base_sha,
        "roundIsCurrentHead": anchor == info["headRefOid"],
        "cleanReviewAtHead": clean_at_head,
        "reviewExists": False,
        "triaged": False,
        "source": str(round_dir / "source.md"),
        "roundDir": str(round_dir),
        **round_identity(fresh, fresh_reviews),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    f = sub.add_parser("fetch", help="fetch the latest round of raw GitHub comments to disk")
    f.add_argument("--pr", required=True)
    f.add_argument("--repo", default=str(REPO_ROOT))
    f.add_argument("--lane", default="github")

    sub.add_parser("reviews", help="each reviewer's last reviewed commit and current HEAD coverage").add_argument(
        "--pr", required=True
    )

    sub.add_parser(
        "verdict", help="verdict of the latest review body on the current HEAD and count of unresolved remarks"
    ).add_argument("--pr", required=True)

    rp = sub.add_parser("reply", help="reply a verdict to that finding's own thread")
    rp.add_argument("--pr", required=True)
    rp.add_argument("--comment-id", required=True)
    rp.add_argument("--body-file", required=True)
    rp.add_argument(
        "--round-dir",
        default=None,
        help="record that this item was replied to, so a rerun after a mid-run failure skips it",
    )

    t = sub.add_parser("post-triage", help="post the triage ledger back to the PR verbatim")
    t.add_argument("--pr", required=True)
    t.add_argument("--triage", required=True)
    t.add_argument(
        "--round-dir",
        default=None,
        help="after a successful post, write the disposition-complete marker in that round directory",
    )

    args = parser.parse_args(argv)
    try:
        if args.command == "fetch":
            payload = fetch(args.pr, Path(args.repo), args.lane)
        elif args.command == "reviews":
            payload = review_coverage(args.pr)
        elif args.command == "verdict":
            payload = head_verdict(args.pr)
        elif args.command == "reply":
            payload = reply_to_finding(
                args.pr, args.comment_id, Path(args.body_file), Path(args.round_dir) if args.round_dir else None
            )
        else:
            payload = post_triage(args.pr, Path(args.triage), Path(args.round_dir) if args.round_dir else None)
        print(json.dumps(payload, ensure_ascii=False, indent=2))
        return 0
    except LaneError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
