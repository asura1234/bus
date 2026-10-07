"""GitHub reads/writes for github_review_lane: fetch comments/reviews, coverage and verdict queries,
post the ledger back, and per-finding replies.

Split out of github_review_lane.py with the logic unchanged verbatim; the CLI entry point is still
github_review_lane.py.
"""

from __future__ import annotations

import json
import os
from collections import OrderedDict
from pathlib import Path

from review_lane_core import REPLIED_MARKER, SECRET_RE, LaneError, _gh
from review_lane_rounds import classify_review, review_is_clean, review_stamp, write_triaged_marker


def pr_view(pr: str) -> dict:
    data = json.loads(
        _gh(
            "pr",
            "view",
            pr,
            "--json",
            "number,headRefName,headRefOid,baseRefName,isDraft,url",
        )
    )
    if not data.get("headRefOid"):
        raise LaneError(f"PR {pr} 没有 headRefOid")
    return data


def fetch_review_bodies(pr: str) -> list[dict]:
    """Review bodies awaiting disposition, sorted by submission time. Bodies with a clean verdict and no
    findings section are excluded."""

    raw = json.loads(
        _gh(
            "api",
            f"repos/{{owner}}/{{repo}}/pulls/{pr}/reviews?per_page=100",
            "--paginate",
        )
    )
    # Two reviews on the same HEAD can have **the same second-level** `submitted_at`; sorting by time alone
    # falls back to REST's return order, so an earlier verdict may sort later. Use id as a stable secondary key.
    raw.sort(key=lambda r: (review_stamp(r), r["id"]))
    return [r for r in raw if classify_review(r)["outstanding"]]


def fetch_rounds(pr: str) -> "OrderedDict[str, dict]":
    """A round = inline comments pinned to the same commit **and** the review bodies awaiting disposition.

    The two streams must be merged into the same anchor group: they come from the same review. Separating them
    would make the same review get dispositioned twice, and a body forming its own round could not read the
    context from the inline-comment half.
    """

    raw = json.loads(
        _gh(
            "api",
            f"repos/{{owner}}/{{repo}}/pulls/{pr}/comments?per_page=100",
            "--paginate",
        )
    )
    raw.sort(key=lambda c: c["created_at"])
    rounds: "OrderedDict[str, dict]" = OrderedDict()
    for comment in raw:
        # A thread **reply** is not a finding; it is a response to a finding. Without excluding it the flow
        # becomes an infinite loop: this skill's own verdict replies are inline comments carrying
        # `in_reply_to_id`, so the next fetch would treat one as a new finding with a new id and open the next
        # round, reply again after disposition, and never converge.
        if comment.get("in_reply_to_id") is not None:
            continue
        anchor = comment.get("original_commit_id") or comment.get("commit_id")
        if anchor:
            rounds.setdefault(anchor, {"comments": [], "reviews": []})["comments"].append(comment)
    for review in fetch_review_bodies(pr):
        anchor = review.get("commit_id")
        if anchor:
            rounds.setdefault(anchor, {"comments": [], "reviews": []})["reviews"].append(review)
    return rounds


def review_coverage(pr: str) -> dict:
    """The commit of each reviewer's last submitted review, and whether the current HEAD is covered.

    "This round has no inline comments" and "the reviewer hasn't looked at this tree yet" look identical on the
    comments API — both are zero items. But the review event itself carries a commit, so it can be decided: a
    reviewer who has reviewed this PR but not the current HEAD is "not their turn yet", not "looked and had no
    comments". Without this, merging right after a push amounts to bypassing review.
    """

    data = json.loads(_gh("pr", "view", pr, "--json", "headRefOid,reviews,author"))
    head = data["headRefOid"]
    # The author is not a reviewer. Replying to a review comment creates a new review **in the author's name**,
    # so after a few rounds of disposition the author becomes a "re-reviewer who reviewed >= 2 commits", forever
    # hanging in `pendingReviewers`, and `coveredAtHead` is forever false — merge-pr then waits for an event that
    # by definition never happens. Measured on this PR: 18 reviews under the author's name, covering 6 commits,
    # all from verdict replies.
    author = (data.get("author") or {}).get("login")
    latest: dict[str, dict] = {}
    seen_commits: dict[str, set[str]] = {}
    for review in data.get("reviews") or []:
        login = (review.get("author") or {}).get("login")
        commit = (review.get("commit") or {}).get("oid")
        if not login or not commit or login == author:
            continue
        seen_commits.setdefault(login, set()).add(commit)
        prior = latest.get(login)
        if prior is None or review["submittedAt"] >= prior["submittedAt"]:
            latest[login] = {"commit": commit, "submittedAt": review["submittedAt"]}

    # Only reviewers who have **proven they re-review** count as "not their turn yet".
    # The criterion is that it reviewed >= 2 distinct commits on this PR. Measured: Copilot re-reviews on every
    # push (once each on 4 commits), while codex reviewed only once when the PR was opened and skipped all three
    # later pushes. Requiring "all reviewers cover the current HEAD" never holds under such a combination, and
    # waiting inevitably runs to timeout — that isn't caution, it's writing the gate as a deadlock.
    recurring = {k for k, v in seen_commits.items() if len(v) >= 2}
    pending = sorted(k for k in recurring if latest[k]["commit"] != head)
    once_only = sorted(k for k in latest if k not in recurring and latest[k]["commit"] != head)

    # Two **independent** signals that must not be merged into one:
    #   headHasReview — someone actually reviewed the current HEAD;
    #   pendingReviewers — those proven to re-review are still behind.
    # Merging them opens this hole: when the whole PR has only one reviewer who reviewed once and stopped on an
    # old commit, `recurring` is empty → `pending` is empty → "covered" is true, while nobody reviewed the
    # current HEAD.
    head_has_review = any(v["commit"] == head for v in latest.values())
    return {
        "pr": int(pr),
        "head": head,
        "reviewers": {k: v["commit"][:12] for k, v in latest.items()},
        "recurringReviewers": sorted(recurring),
        "pendingReviewers": pending,
        # Reviewed once and never followed up: report it truthfully, but don't block — it hasn't shown it
        # re-reviews.
        "reviewedOnceAt": {k: latest[k]["commit"][:12] for k in once_only},
        "headHasReview": head_has_review,
        "coveredAtHead": head_has_review and not pending,
        "everReviewed": bool(latest),
    }


def head_verdict(pr: str) -> dict:
    """The verdict of the **latest** review body on the current HEAD, for the merge gate to judge directly.

    The merge gate can't look only at "we finished dispositioning this round": that is local ledger state, saying
    the author did their homework, which is a separate matter from whether the reviewer agrees. After one round is
    dispositioned `triaged` becomes true, so a PR still saying
    `🔵 Needs a closer look` / `Open (3)` gets merged anyway — the reviewer explicitly said there are unresolved
    findings, while the gate looks at our own marker. So separately provide **the reviewer's own conclusion**.

    Only reviews on the current HEAD count: an approval on an old commit does not constitute approval of the tree
    about to be merged.
    """

    head = pr_view(pr)["headRefOid"]
    raw = json.loads(
        _gh(
            "api",
            f"repos/{{owner}}/{{repo}}/pulls/{pr}/reviews?per_page=100",
            "--paginate",
        )
    )

    # Can't filter by "non-empty body": `APPROVED` / `CHANGES_REQUESTED` can have **a state but no body**, and
    # those are exactly the two conclusions human reviewers give most often; filtering them out would make an
    # explicit approval or an explicit change request vanish entirely here.
    # `DISMISSED` must be excluded, though: when it carries a body it sorts last, so the reported "current verdict"
    # is a review that has already been withdrawn, while the one actually in effect is masked.
    # Replying to a review comment **creates a new empty-body `COMMENTED` review**. It sorts last by time,
    # so a single reply by the author masks the reviewer's real verdict (measured: after replying, `verdict`
    # became None).
    # An empty-body `COMMENTED` carries no conclusion, so exclude it; `APPROVED` / `CHANGES_REQUESTED` are
    # explicit conclusions even without a body, so keep them.
    def carries_verdict(r: dict) -> bool:
        state = (r.get("state") or "").upper()
        if state == "DISMISSED":
            return False
        if state in {"APPROVED", "CHANGES_REQUESTED"}:
            return True
        return bool((r.get("body") or "").strip())

    at_head = [r for r in raw if r.get("commit_id") == head and carries_verdict(r)]
    at_head.sort(key=lambda r: (review_stamp(r), r["id"]))
    if not at_head:
        return {
            "pr": int(pr),
            "head": head,
            "hasReviewAtHead": False,
            "verdict": None,
            "clean": False,
            "outstandingFindings": None,
            "approved": False,
        }

    latest = at_head[-1]
    verdict = classify_review(latest)
    body = latest.get("body") or ""
    clean = review_is_clean(body)
    # Approval must satisfy two things at once, and both must go through `classify_review`, not just a green
    # heading plus a count:
    #   - No outstanding conclusion at all (it already accounts for CHANGES_REQUESTED, findings sections, and
    #     state/body contradictions). Bypassing it, a `CHANGES_REQUESTED` with a green heading in its body would
    #     be judged approved;
    #   - A **positive** approval signal: the REST state is `APPROVED`, or the body gives a clean verdict. Without
    #     this, an empty-body `COMMENTED` (just the envelope for inline comments) would be treated as approval
    #     because it has "no outstanding conclusion".
    approved = (not verdict["outstanding"]) and (clean or verdict["state"] == "APPROVED")
    return {
        "pr": int(pr),
        "head": head,
        "hasReviewAtHead": True,
        "reviewId": latest["id"],
        "submittedAt": review_stamp(latest),
        "state": verdict["state"],
        "verdict": verdict["headline"],
        "clean": clean,
        # Summed across "Open", "Previously missed", and "Suppressed comments", which isn't the same as the
        # `Open (N)` number in the UI; the gate only cares whether it is 0, so the summed measure suffices and is
        # more fail-closed.
        # Reviews with only a verdict and no findings section (`🔵 Needs a closer look`, an empty-body
        # `CHANGES_REQUESTED`) have `findings` of 0, but they really are outstanding. Reporting 0 would make
        # callers read a self-contradictory state of "outstanding but the unresolved count is 0", so report at
        # least 1.
        "outstandingFindings": max(verdict["findings"], 1 if verdict["outstanding"] else 0),
        "outstanding": verdict["outstanding"],
        "approved": approved,
    }


def post_triage(pr: str, triage: Path, round_dir: Path | None = None) -> dict:
    """Post the verdict ledger back to the PR verbatim.

    A machine reviewer can raise a rejected finding again at any time — it has no memory, it only sees that the
    code hasn't changed. Only with the ledger posted on the PR does "why this one isn't changed" live in the same
    place as the code, so the next round (human or machine) can see that reason.
    """

    if not triage.is_file():
        raise LaneError(f"triage 台账不存在：{triage}")
    text = triage.read_text(encoding="utf-8")
    if not text.strip():
        raise LaneError(f"triage 台账是空的：{triage}")

    hits = sorted({m.group(0) for m in SECRET_RE.finditer(text)})
    if hits:
        raise LaneError(f"台账里有疑似凭据或本机绝对路径，拒绝贴到公开 PR：{hits[:5]}。先脱敏再重试")

    limit = 65536
    if len(text.encode("utf-8")) > limit:
        raise LaneError(
            f"台账超过 GitHub 单条评论上限（{limit} 字符）。按 disposition 分段发多条，"
            "不要截断——被截掉的总是排在后面的 REJECT/FLAG"
        )

    comment = _gh("pr", "comment", pr, "--body-file", str(triage)).strip()
    result = {"pr": pr, "triage": str(triage), "comment": comment}

    # The marker is written only after posting succeeds: if posting fails (credential hit, too long, gh failure)
    # this round simply didn't finish, and it must not leave behind a file claiming it did.
    if round_dir is not None:
        if not round_dir.is_dir():
            raise LaneError(f"round 目录不存在：{round_dir}")
        marker = write_triaged_marker(round_dir, {"pr": pr, "comment": comment, "triage": str(triage)})
        result["triagedMarker"] = str(marker)
    return result


def reply_to_finding(pr: str, comment_id: str, body: Path, round_dir: Path | None = None) -> dict:
    """Reply with a verdict inside **that finding's own thread**.

    Measured (PR #899, all 18 threads hit): after replying to a review comment, the machine reviewer marks that
    finding as resolved — including ones with **no corresponding fix commit** and `outdated == false`. In other
    words, an "author rejection" can be received by it, and the channel is a thread reply.

    Posting a standalone PR comment (the ledger) can't achieve this: that is an issue comment, not on any review
    thread, and the machine reviewer won't read it. The ledger still has to be posted, but that is for humans and
    for leaving an audit trail, not a means of getting the verdict to the reviewer.
    """

    text = body.read_text(encoding="utf-8")
    if not text.strip():
        raise LaneError(f"回复正文是空的：{body}")
    hits = sorted({m.group(0) for m in SECRET_RE.finditer(text)})
    if hits:
        raise LaneError(f"回复里有疑似凭据或本机绝对路径，拒绝贴到公开 PR：{hits[:5]}。先脱敏再重试")
    # Persist "this one has already been replied to" per item. `triaged.json` is only written at the very last
    # step of the round, so rerunning after any reply or ledger post fails midway would **reply again** to the ones
    # that already succeeded, piling up duplicate verdicts on the review thread.
    done_file = round_dir / REPLIED_MARKER if round_dir is not None else None
    replied: dict[str, str] = {}
    if done_file is not None and done_file.is_file():
        try:
            replied = json.loads(done_file.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            replied = {}
        if str(comment_id) in replied:
            return {"pr": pr, "commentId": comment_id, "reply": replied[str(comment_id)], "skipped": "already-replied"}

    out = _gh(
        "api",
        "-X",
        "POST",
        f"repos/{{owner}}/{{repo}}/pulls/{pr}/comments/{comment_id}/replies",
        "-F",
        f"body=@{body}",
        "--jq",
        ".html_url",
    )
    url = out.strip()
    if done_file is not None:
        replied[str(comment_id)] = url
        tmp = done_file.with_suffix(done_file.suffix + ".tmp")
        tmp.write_text(json.dumps(replied, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, done_file)
    return {"pr": pr, "commentId": comment_id, "reply": url}
