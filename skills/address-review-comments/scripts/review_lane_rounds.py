"""Pure-computation part of github_review_lane: review-body verdict classification, round identity, and
source.md rendering.

Split out of github_review_lane.py with the logic unchanged verbatim; the CLI entry point is still
github_review_lane.py.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
from collections import OrderedDict
from pathlib import Path

from review_lane_core import ROUND_DIR_RE, TRIAGED_MARKER


# The review **body** is a stream of findings parallel to inline comments, not their cover page. Measured on
# this repo's machine reviewer: the body carries a verdict line (`🟢 Approval recommended` /
# `🟡 Changes recommended` / `🔵 Needs a closer look`), and concrete findings can live **only** in the body's
# `Suppressed comments (N)` / `Previously missed (N)` sections, with not a single new inline comment (the body
# itself says `Comments generated: 0`). Reading only inline comments, such a round looks identical on the
# comments API to "looked and had no comments", so it gets let through as a clean re-review — while it is exactly
# the round in which the reviewer explicitly said "this tree needs another look".
#
# The verdict line **isn't necessarily the first heading**. Measured: bodies in the `ccr-overview-v2` layout first
# open a `## Copilot review overview`, with the verdict written beneath it as `### 🟡 Changes recommended`.
# Looking only at the first heading would read every v2 body's verdict as `Copilot review overview` — so clean
# re-reviews also go unrecognized, and a round is opened out of nothing every time. So the criterion is to
# **scan all headings**, and it is clean only if some heading matches a clean verdict.
#
# "Clean" is **positively enumerated** only and must match the **whole line**. Writing it as "contains" would make
# `Disapproval recommended`, `Not approval recommended`, and `🟢 Changes recommended` all judged clean — letting
# anything through as long as it happens to carry a clean word, which makes fail-closed a mere ornament. An
# unrecognizable verdict line always counts as outstanding: this set of markers is owned by an external bot, and
# when it changes its wording, treating unknown as clean would silently skip an entire round of review, whereas
# treating unknown as outstanding at most adds one extra verdict and leaves a trace in the ledger.
# The direction is chosen by consequence; it is the same reasoning as the SECRET_RE line below: "prefixes are an
# enumeration; miss one and that whole class is let through".
CLEAN_VERDICT_RE = re.compile(r"^(?:🟢\s*)?approval\s+recommended$", re.IGNORECASE)

# The verdict line's own marker, used only to **display** which line to cite (persisted for transcription), not
# part of the clean-or-not decision.
# Without 🟣, a 🟣 verdict would be reported as the body's first heading — in the v2 layout that is
# `Copilot review overview`, which amounts to misreporting the verdict's name.
VERDICT_MARK_RE = re.compile(r"[🟢🟡🔵🟣]")

# Explicit **non**-approval markers. Used in only one place: when the GitHub state says approved but the body
# says 🟡 / 🔵, the two contradict, and by fail-closed it is classified as outstanding. We can't conversely
# require an APPROVED body to contain a clean verdict — that set of markers is the machine reviewer's
# vocabulary, humans commonly write a paragraph of ordinary remarks when approving, and insisting on it would turn
# every human approval into an outstanding round.
NON_CLEAN_MARK_RE = re.compile(r"[🟡🔵]")

# "Does this line look like a verdict" — it starts with a non-alphanumeric, non-whitespace symbol (🟢🟡🔵 and any
# marker added in the future).
# Enumerating only the known non-approval markers isn't enough: an **unrecognizable structured verdict** like
# `### 🟣 Something new` would neither count as a conflict nor block a green heading in the same body, so the
# whole round would be judged clean — exactly the class fail-closed is meant to prevent.
VERDICT_LIKE_RE = re.compile(r"^[^\w\s]")

# Findings sections hanging in the body. Three forms measured: `### Suppressed comments (2)`,
# `Previously missed (1) — in code that hasn't changed since the last review.`, and the v2 overview's
# `<summary><strong>Open (3)</strong></summary>` — `Open (N)` is the reviewer's own count of "findings not yet
# resolved", precisely the authoritative count for judging whether it thinks this tree still has problems. A
# section with a count of 0 doesn't count as findings.
BODY_FINDINGS_RE = re.compile(r"(?:suppressed\s+comments|previously\s+missed|open)\s*\((\d+)\)", re.IGNORECASE)


def review_headings(body: str) -> list[str]:
    """Text of all markdown headings in the body, **skipping fenced code blocks**.

    A `### 🟢 Approval recommended` inside a code block is a **quoted** literal, not this review's verdict.
    Reading it as a heading would make a review whose verdict is 🟡 be judged clean — so the whole round of
    findings is silently skipped, and this is exactly the body shape most likely to appear when a review discusses
    the verdict markers themselves (e.g. when it is reviewing this lane's own code).
    """

    headings: list[str] = []
    fence_char: str | None = None
    fence_len = 0
    for raw in body.splitlines():
        expanded = raw.expandtabs(4)
        line = expanded.strip()
        indent = len(expanded) - len(expanded.lstrip(" "))
        if fence_char is not None:
            # A closing fence must be **the same character, no shorter than the opening fence, and without an
            # info string**. Storing only the first three characters and checking with startswith would treat
            # lines like ```` ```python ```` and a triple-backtick inside a four-backtick fence as closing, so the
            # `### 🟢 Approval recommended` after it turns back into a "real verdict".
            if indent < 4 and line and set(line) == {fence_char} and len(line) >= fence_len:
                fence_char = None
            continue
        if indent < 4 and (line.startswith("```") or line.startswith("~~~")):
            char = line[0]
            run = len(line) - len(line.lstrip(char))
            info = line[run:]
            # The info string of a backtick fence may not contain backticks (CommonMark).
            if run >= 3 and not (char == "`" and "`" in info):
                fence_char, fence_len = char, run
                continue
        # Indentation of 4 or more spaces is an **indented code block**, not a heading: CommonMark ATX headings
        # are indented at most 3 spaces.
        # Blocking fences but not indentation leaves the other half of the same hole open.
        if len(expanded) - len(expanded.lstrip(" ")) >= 4:
            continue
        if not line.startswith("#"):
            continue
        # An ATX heading's `#` must be followed by whitespace (or the line is only `#`), otherwise things like
        # `#hashtag` and `#123` would also be read as headings.
        rest = line.lstrip("#")
        # CommonMark ATX headings have at most 6 `#`. Without the cap, `####### 🟢 Approval recommended`
        # would be treated as a clean verdict even though it isn't a heading at all — yet another path around
        # fail-closed.
        if len(line) - len(rest) > 6:
            continue
        if rest and not rest[:1].isspace():
            continue
        headings.append(rest.strip())
    return headings


def review_headline(body: str) -> str | None:
    """The verdict line displayed when persisting: prefer a heading with a verdict marker, otherwise fall back to
    the first heading.

    Used only for the heading line of `source.md`, so the transcriber can see the review's conclusion at a glance.
    Clean-or-not is decided independently by `review_is_clean`, which doesn't read this value — which line is
    displayed and whether to let it through are two separate matters.
    """

    headings = review_headings(body)
    for heading in headings:
        if VERDICT_MARK_RE.search(heading):
            return heading
    return headings[0] if headings else None


def review_is_clean(body: str) -> bool:
    """Whether the verdict given by the body is **unique and clean**.

    Asking only "is there a clean heading somewhere" gets fooled by quoted passages: when the body has both a real
    `### 🟡 Changes recommended` and a `### 🟢 Approval recommended` in a history section (and not inside a code
    block), the whole round would be judged clean and silently skipped. A verdict conflict is one kind of
    "unrecognizable", and by fail-closed it is classified as not clean.
    """

    # A clean verdict may also come without an emoji (`### Approval recommended`). Collecting only by "the first
    # character is a symbol", this form would collect no verdict at all, so `not verdicts` judges it not clean — a
    # clean re-review would instead open a round.
    verdicts = [h for h in review_headings(body) if VERDICT_LIKE_RE.match(h) or CLEAN_VERDICT_RE.match(h)]
    if not verdicts:
        return False
    # If any verdict line isn't a clean verdict (non-approval, or not recognizable at all), the whole body is not
    # clean.
    return all(CLEAN_VERDICT_RE.match(h) for h in verdicts)


def body_finding_count(body: str) -> int:
    """The number of findings the body itself declares (`Suppressed comments (N)` / `Previously missed (N)`)."""

    return sum(int(m.group(1)) for m in BODY_FINDINGS_RE.finditer(body))


def classify_review(review: dict) -> dict:
    """Whether this review's **body** is a stream of findings awaiting disposition.

    The criteria are ordered by consequence, not by field order:
      - `DISMISSED` → no: it has already been withdrawn;
      - `CHANGES_REQUESTED` → yes, **even if the body is empty**. It is itself a blocking conclusion; the findings
        may all be in inline comments or even only in the heading; filtering it out as "empty body" amounts to
        making a review that explicitly requests changes (a state only human reviewers can give) vanish entirely
        from this stream;
      - findings sections in the body → yes, even if the verdict line says approval — those findings live in the
        body, and there is nowhere else to read them;
      - empty body → no: it is just the envelope for inline comments;
      - `APPROVED` → use GitHub's own machine-decidable conclusion, but when 🟡 / 🔵 appear in the body the state
        contradicts the body, and by fail-closed it is classified as outstanding;
      - everything else (`COMMENTED` and any state added in the future) falls to the verdict line; unrecognizable
        means outstanding.
    """

    body = (review.get("body") or "").strip()
    state = (review.get("state") or "").upper()
    headline = review_headline(body)
    findings = body_finding_count(body)
    if state == "DISMISSED":
        outstanding = False
    elif state == "CHANGES_REQUESTED":
        outstanding = True
    elif findings:
        outstanding = True
    elif not body:
        outstanding = False
    elif state == "APPROVED":
        headings = review_headings(body)
        if any(VERDICT_LIKE_RE.match(h) for h in headings):
            # The body uses the **review format** (headings carry verdict markers): then it must give a clean
            # verdict, otherwise by fail-closed it is judged outstanding. Blocking only 🟡 / 🔵 isn't enough — a
            # structured but non-approval verdict like `🟢 Changes recommended` would slip through the gap.
            outstanding = not review_is_clean(body)
        else:
            # Not a single verdict marker = ordinary remarks written by a human when approving; let it through per
            # GitHub's state.
            outstanding = False
    else:
        outstanding = not review_is_clean(body)
    return {"headline": headline, "findings": findings, "state": state, "outstanding": outstanding}


def review_stamp(review: dict) -> str:
    """Time of the review body. REST doesn't provide a review's `updated_at` — this value doesn't change a bit
    when the body is edited, so it can only be used for sorting; recognizing "the body changed" relies on the body
    itself in the digest."""

    return review.get("submitted_at") or ""


def round_activity(group: dict) -> str:
    """The last time this group had any activity; the union of both streams."""

    stamps = [(c.get("updated_at") or c["created_at"]) for c in group["comments"]]
    stamps += [review_stamp(r) for r in group["reviews"]]
    return max(stamps)


def anchored_line(c: dict) -> int | None:
    """The comment's line number in **the commit it is pinned to**.

    `line` is re-resolved by GitHub relative to the current HEAD and drifts with every push; `original_line` is the
    line number in the tree when the comment was written, paired with `original_commit_id` (i.e. this round's
    anchor), and never changes. The position must come from the same tree as the anchor, otherwise the heading
    line of source.md would claim a position that doesn't exist at the anchor.
    """

    line = c.get("original_line")
    return line if line is not None else c.get("line")


def write_triaged_marker(round_dir: Path, payload: dict) -> Path:
    """Atomic persist: first write a temp file in the same directory, then swap it in with `os.replace`.

    A direct `write_text` interrupted halfway leaves a **half-written file**, and downstream treats any file it
    sees as valid. `os.replace` is atomic within the same filesystem: it is either the old state or the complete
    new file, with no in-between state.
    """

    target = round_dir / TRIAGED_MARKER
    tmp = round_dir / (TRIAGED_MARKER + ".tmp")
    tmp.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    os.replace(tmp, target)
    return target


def round_is_triaged(round_dir: Path) -> bool:
    """Whether this round has been fully dispositioned — the marker must **prove itself by content**, not just by
    whether the file exists.

    An empty file, truncated JSON, or an object missing `comment` doesn't count: `comment` is the URL of the comment
    the ledger was posted as, i.e. verifiable evidence that "this round really finished". A marker that can't
    produce evidence is treated as not finished (fail-closed).
    """

    marker = round_dir / TRIAGED_MARKER
    if not marker.is_file():
        return False
    try:
        data = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    return isinstance(data, dict) and bool(data.get("comment"))


def latest_anchor(rounds: "OrderedDict[str, list]") -> str:
    """Pick the group with the **most recent activity**, not the last group in insertion order.

    `setdefault` places a key at the end only the first time it appears; appending comments to the same anchor
    later doesn't move it to the back. So when "a new comment is added to an older anchor" or "a comment on an old
    anchor is edited", `list(rounds)[-1]` still points to the newer anchor that appeared earlier, and the latest
    activity is silently ignored.
    Edits are especially fatal: `content_digest` includes `updated_at` precisely to recognize edited comments, yet
    that logic would never execute at all — this group is never selected.
    """

    return max(rounds, key=lambda a: round_activity(rounds[a]))


def content_digest(comments: list[dict], reviews: list[dict] = ()) -> str:
    """Canonical digest of a whole round's content: id + update time + position + body.

    Comparing only the id set isn't "judging by content": when an existing inline comment is edited on GitHub, the
    id stays the same and the body changes, so that updated finding would be treated as "already fetched" and never
    enter a verdict.

    The position can only come from `original_line`. Using the re-resolved `line` would make the digest **change
    with HEAD**, while the entire reason the digest exists is that "the same group of comments counts as the same
    round on any HEAD". Measured: after a push, this PR's 2 comments had `line` drift from 44/212 to 47/223, with
    id, body, and `updated_at` not changing a bit, yet the digest changed, so a round of comments **already given
    verdicts and posted back** was reopened as a new round.
    """

    digest = hashlib.sha256()
    for c in sorted(comments, key=lambda c: c["id"]):
        digest.update(f"{c['id']}\0{c.get('updated_at') or c['created_at']}\0".encode())
        digest.update(f"{c.get('path')}\0{anchored_line(c)}\0".encode())
        digest.update(((c.get("body") or "") + "\0").encode())
    # When there are no review bodies, **not a single byte is appended**: the digest must be byte-identical to the
    # old implementation that only looked at inline comments. Otherwise, the moment this upgrade lands, every
    # dispositioned round of comments on disk would be reopened, re-given verdicts, and have its ledger reposted
    # because the digest changed — exactly what "the same group of comments counts as the same round at any time"
    # is meant to prevent.
    for r in sorted(reviews, key=lambda r: r["id"]):
        # The body itself must go into the digest: `submitted_at` doesn't change when the body is edited; the body
        # is the only thing that changes with edits.
        digest.update(f"{r['id']}\0{review_stamp(r)}\0".encode())
        digest.update(((r.get("body") or "") + "\0").encode())
    return digest.hexdigest()


def item_key(kind: str, item: dict) -> str:
    return f"{kind}{item['id']}"


def item_digests(comments: list[dict], reviews: list[dict] = ()) -> dict[str, str]:
    """Per-item content digests, persisted into source.json for the next round to compute the delta.

    The delta used to be judged by **id**, which made "this item was changed" and "this item is unchanged" look
    identical: if the id is in `seen` it gets subtracted, so a rewritten finding would never enter any round. After
    judging by content, a changed item is new by itself; an unchanged one is still recognized as already
    dispositioned.
    """

    digests: dict[str, str] = {}
    for c in comments:
        digest = hashlib.sha256()
        digest.update(f"{c['id']}\0{c.get('updated_at') or c['created_at']}\0".encode())
        digest.update(f"{c.get('path')}\0{anchored_line(c)}\0".encode())
        digest.update(((c.get("body") or "") + "\0").encode())
        digests[item_key("c", c)] = digest.hexdigest()
    for r in reviews:
        digest = hashlib.sha256()
        digest.update(f"{r['id']}\0{review_stamp(r)}\0".encode())
        digest.update(((r.get("body") or "") + "\0").encode())
        digests[item_key("r", r)] = digest.hexdigest()
    return digests


def round_identity(comments: list[dict], reviews: list[dict] = ()) -> dict:
    """A round's identity: the id sets of both streams + the latest timestamp + the content digest."""

    stamps = [(c.get("updated_at") or c["created_at"]) for c in comments]
    stamps += [review_stamp(r) for r in reviews]
    return {
        "commentIds": sorted(c["id"] for c in comments),
        "reviewIds": sorted(r["id"] for r in reviews),
        "latestAt": max(stamps),
        "contentDigest": content_digest(comments, reviews),
        "itemDigests": item_digests(comments, reviews),
        "commentCount": len(comments),
        "reviewCount": len(reviews),
    }


def existing_rounds(lane_dir: Path) -> list[tuple[int, Path]]:
    if not lane_dir.is_dir():
        return []
    found = []
    for child in lane_dir.iterdir():
        match = ROUND_DIR_RE.match(child.name)
        if match and (child / "source.json").is_file():
            found.append((int(match.group(1)), child))
    return sorted(found)


def render_source(pr: dict, anchor: str, comments: list[dict], reviews: list[dict] = ()) -> str:
    """Persist the original text verbatim, with positions and timestamps. This is what the agent reads when
    transcribing review.md.

    Review bodies, like inline comments, are dumped **whole and verbatim**, not summarized here and with no findings
    sections picked out here: which sentence is a finding, which dimension it falls under, and which code it points
    to are judgments to be understood during transcription; the script can only move the text to another place.
    """

    lines = [
        f"# PR #{pr['number']} GitHub 评审评论 — round anchored at {anchor[:12]}",
        "",
        f"- 分支：{pr['headRefName']} @ {pr['headRefOid'][:12]}",
        f"- anchor commit：`{anchor}`",
        f"- inline 评论数：{len(comments)}",
        f"- 待处置评审正文数：{len(reviews)}",
        "",
        "> 以下为 GitHub 原文逐字转存，未做任何改写。",
        "",
    ]
    for index, c in enumerate(comments, 1):
        line_no = anchored_line(c)
        lines += [
            f"## {index}. {c['user']['login']} — `{c.get('path')}:{line_no}`",
            "",
            f"- comment id：{c['id']}",
            f"- 时间：{c['created_at']}",
            "",
            c.get("body") or "",
            "",
        ]
    for index, r in enumerate(reviews, 1):
        verdict = classify_review(r)
        lines += [
            f"## 评审正文 {index}. {r['user']['login']} — 裁定：{verdict['headline'] or '（无裁定行）'}",
            "",
            f"- review id：{r['id']}",
            f"- 时间：{review_stamp(r)}",
            f"- state：{verdict['state'] or '（无）'}",
            f"- 正文自述意见数：{verdict['findings']}",
            "",
            r.get("body") or "",
            "",
        ]
    return "\n".join(lines)
