"""The lower layer of github_review_lane: lane identity, credential blocking, gh/git calls.

Split out of github_review_lane.py with the logic unchanged verbatim; the CLI entry point is still
github_review_lane.py.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path


ROUND_DIR_RE = re.compile(r"^round-(\d+)$")
LANE_RE = re.compile(r"^[A-Za-z0-9_-]+$")
# The only evidence that a round has been **truly fully dispositioned**. The existence of `review.md` only shows
# that PASS 0a transcribed it, and transcription happens **before** verification, adjudication, fixing and
# landing -- if the run STOPs midway on a FLAG or crashes, review.md is still there. Using it as the completion
# criterion would let a round's unadjudicated comments be treated as dispositioned and merged. Posting the ledger
# back to the PR is the last step of the whole flow, so the marker is written at that step; only then does it
# really mean "this round is done".
TRIAGED_MARKER = "triaged.json"
# Completion marker for per-item verdict replies. Kept separate from TRIAGED_MARKER: that marker is written only
# once per round, at the last step, so it cannot help when a mid-run failure is rerun, and duplicate replies pile
# up directly on the review thread.
REPLIED_MARKER = "replied.json"


def safe_lane(lane: str) -> str:
    """The lane name is concatenated directly into the artifact path, so it must be a **single-segment** safe name.

    Without validation, `--lane ../../outside` would write the round outside `temp/review-pr/<slug>`,
    and the isolation boundary of review artifacts would be gone. This is a fail-closed gate: out of bounds means
    rejection, no sanitizing -- sanitizing would make the name passed in differ from the name written to disk.
    """

    if not LANE_RE.match(lane):
        raise LaneError(f"lane 名只允许单段 [A-Za-z0-9_-]，拒绝：{lane!r}")
    return lane


# The ledger is a local artifact under temp/, while the PR is public. Block credentials and local absolute paths
# before posting.
SECRET_RE = re.compile(
    # GitHub token prefixes must be **listed exhaustively**: missing one means that class can be posted in the clear.
    # ghp_ personal access token / gho_ OAuth / ghu_ user-to-server / ghs_ server-to-server /
    # ghr_ refresh / github_pat_ fine-grained PAT.
    r"\b(?:ghp_|gho_|ghu_|ghs_|ghr_|github_pat_)[A-Za-z0-9_]{16,}"
    r"|\b(?:api[_-]?key|secret|password|token)\s*[:=]\s*\S+"
    r"|ssh://\S+"
    # `file://` needs its own block: what follows it is a **local path**, and the absolute-path rule's negative
    # lookbehind contains `/`; in `file:///Users/alice/x` the char before `/Users` is exactly `/`, so that rule
    # never matches it. http(s) is not on this list -- those are remote addresses the ledger is supposed to cite.
    r"|file:/\S*"
    # Block absolute paths **as a whole class**, not by enumerating prefixes. Enumerating `/Users`, `/home` and the
    # like inevitably misses some: `/tmp`, `/private/var`, `/opt`, `/Volumes`, drive-letter paths outside user dirs
    # are not on the list, and what shows up in ledgers is precisely, and often, pytest tmp paths like
    # `/private/var/folders/...`. The ledger should use repo-relative paths (`path:line`) anyway, so "any absolute
    # path" is a safe criterion. The leading negation lets path segments in URLs (`https://host/a/b`) and
    # repo-relative paths through.
    r"|(?<![\w:/~.-])/(?:[A-Za-z0-9._-]+/){1,}[A-Za-z0-9._-]+"
    # `~/…` needs its own entry: the negative lookbehind above explicitly forbids `~`, so home-relative paths never
    # get matched there, yet they still leak the local directory structure.
    r"|(?<![\w:/.-])~/[A-Za-z0-9._-]+"
    r"|(?<![\w\\])[A-Za-z]:\\(?:[^\s\\]+\\)*[^\s\\]+"
    r"|(?<![\w\\])\\\\[A-Za-z0-9._-]+\\[A-Za-z0-9._-]+",
    re.IGNORECASE,
)


class LaneError(RuntimeError):
    """Could not get the signal, or what was obtained is not enough to constitute a round."""


def _gh(*args: str) -> str:
    done = subprocess.run(["gh", *args], capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise LaneError(f"gh {' '.join(args)} 失败：{done.stderr.strip()}")
    return done.stdout


def branch_slug(branch: str) -> str:
    """Slug rule verbatim identical to the review skills; detached uses `_detached_`."""

    return re.sub(r"[^A-Za-z0-9_-]", "-", branch) if branch else "_detached_"


def local_branch(repo: Path) -> str:
    """Name of the locally checked-out branch. On a detached HEAD there is no lane identity to speak of; reject."""

    out = subprocess.run(["git", "-C", str(repo), "branch", "--show-current"], capture_output=True, text=True)
    branch = out.stdout.strip()
    if out.returncode != 0 or not branch:
        raise LaneError(
            "拿不到本地分支名（detached HEAD？）。lane 目录按本地分支建，"
            "没有分支就没有 lane 身份，不要退回用 PR 的 headRefName——"
            "那会写到 address-review-comments 不会 glob 的地方"
        )
    return branch


def base_revision(repo: Path, base_ref_name: str) -> tuple[str, str]:
    """The base's ref name and OID.

    The artifact's `**基线**` must be `<ref> @ <short-sha>`; the parser rejects a structured PR artifact that is
    missing the revision. If only `baseRefName` were written, whoever writes the lane would have to run rev-parse
    separately -- an extra query not written into any doc -- so it is resolved at write time.
    """

    ref = f"origin/{base_ref_name}"
    done = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", ref],
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode != 0:
        raise LaneError(f"解析不了 base：{ref}（{done.stderr.strip()}）")
    return ref, done.stdout.strip()
