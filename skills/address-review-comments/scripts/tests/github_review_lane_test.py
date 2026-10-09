"""Mechanical contract of github_review_lane: round identity, idempotency, and fail-closed before posting back.

The two things pinned here each correspond to a lesson learned in practice:
  - Idempotency must be judged by **content**. A machine review can rerun on the same commit, with the anchor
    unchanged but the comments new; comparing only the anchor treats the new round as "already fetched", and
    that round will never be dispositioned.
  - The ledger is a local artifact under temp/, while the PR is public. Block credentials and local absolute
    paths before posting, and refuse on a hit rather than auto-rewriting -- rewriting would make what gets
    posted diverge from the ledger.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

import github_review_lane as lane  # noqa: E402
from github_review_lane import LaneError, branch_slug, round_identity  # noqa: E402


HEAD = "a" * 40
ANCHOR = "c" * 40

# The autouse fixture stubs out lane.local_branch; to test the real one, grab it before that happens.
REAL_LOCAL_BRANCH = lane.local_branch


def _pr(head=HEAD):
    return {
        "number": 791,
        "headRefName": "feat/x",
        "headRefOid": head,
        "baseRefName": "master",
        "isDraft": False,
        "url": "https://x.invalid/791",
    }


def _mark_triaged(round_dir) -> None:
    """Mark a round as dispositioned in the exact shape `post_triage` actually writes."""
    lane.write_triaged_marker(Path(round_dir), {"pr": "791", "comment": "https://x/1"})


def _comment(cid, sha, created, body="正文", path="a.py", line=7, original_line=None):
    return {
        "id": cid,
        "user": {"login": "Copilot"},
        "path": path,
        "line": line,
        "original_line": original_line,
        "body": body,
        "created_at": created,
        "original_commit_id": sha,
        "commit_id": sha,
    }


def _patch_everywhere(monkeypatch, name, value):
    """After the split, names like `_gh` are bound separately in several sibling modules; the stub must be
    applied to every module holding it, otherwise calls silently hit the real gh."""
    for module in (lane, *(sys.modules[m] for m in ("review_lane_core", "review_lane_rounds", "review_lane_github"))):
        if name in vars(module):
            monkeypatch.setattr(module, name, value)


@pytest.fixture(autouse=True)
def _stub_base_revision(monkeypatch):
    """tmp_path is not a git repo; the base OID and the local branch name each have dedicated test cases."""
    _patch_everywhere(monkeypatch, "base_revision", lambda repo, name: (f"origin/{name}", "b" * 40))
    _patch_everywhere(monkeypatch, "local_branch", lambda repo: "feat/x")


def _review_body(rid, sha, at, body, state="COMMENTED", login="Copilot"):
    """One entry of REST `pulls/<n>/reviews`: the body carries its own verdict line, and the only time is
    `submitted_at`."""
    return {"id": rid, "user": {"login": login}, "state": state, "commit_id": sha, "submitted_at": at, "body": body}


APPROVED_BODY = "### 🟢 Approval recommended\n\n未发现阻塞批准的问题。"
CLOSER_LOOK_BODY = "### 🔵 Needs a closer look\n\nThe changes require final human review."
CHANGES_BODY = "### 🟡 Changes recommended\n\n落轨闸把整段素材的 clip 拒了。"
SUPPRESSED_BODY = (
    "### 🟡 Changes recommended\n\n"
    "<details><summary>Review details</summary>\n\n"
    "### Suppressed comments (1)\n\n"
    "**native/media/tests/service/http/wire_test.cpp:207**\n"
    "* 这条 cold/hit 检查证明不了实际解码的是哪个时间戳。\n\n"
    "- **Comments generated:** 0\n</details>"
)


def _fake_gh(monkeypatch, comments, pr=None, posted=None, reviews=None, bodies=None):
    def gh(*args):
        if args[0] == "pr" and args[1] == "view":
            # The clean-rereview check in `fetch` fetches reviews once more; both field sets go through the same stub.
            if "reviews" in args[-1]:
                base = pr or _pr()
                return json.dumps({"headRefOid": base["headRefOid"], "reviews": reviews or []})
            return json.dumps(pr or _pr())
        if args[0] == "pr" and args[1] == "comment":
            if posted is not None:
                posted.append(args)
            return "https://x.invalid/791#issuecomment-1\n"
        if args[0] == "api":
            # The two endpoints must be routed separately: if the review-body path received inline comments,
            # they would be treated as bodies without a verdict line, and every test case would gain a phantom
            # round.
            return json.dumps(bodies or [] if "/reviews" in args[1] else comments)
        raise AssertionError(f"unexpected gh call: {args}")

    _patch_everywhere(monkeypatch, "_gh", gh)


def test_branch_slug_matches_review_skills() -> None:
    assert branch_slug("feat/x") == "feat-x"
    assert branch_slug("chore/slim_agent-skills") == "chore-slim_agent-skills"
    assert branch_slug("") == "_detached_"


def test_latest_round_is_selected_and_recorded(tmp_path, monkeypatch) -> None:
    _fake_gh(
        monkeypatch,
        [
            _comment(1, "old" * 13 + "o", "2026-09-17T01:00:00Z"),
            _comment(2, ANCHOR, "2026-09-17T02:00:00Z"),
            _comment(3, ANCHOR, "2026-09-17T02:00:01Z"),
        ],
    )

    result = lane.fetch("791", tmp_path, "github")

    assert result["status"] == "created"
    assert result["round"] == 1
    assert result["anchor"] == ANCHOR
    assert result["commentCount"] == 2
    assert result["latestAt"] == "2026-09-17T02:00:01Z"
    source = Path(result["source"])
    assert source.is_file() and "正文" in source.read_text(encoding="utf-8")


def test_second_fetch_of_the_same_round_is_reused(tmp_path, monkeypatch) -> None:
    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")])

    first = lane.fetch("791", tmp_path, "github")
    second = lane.fetch("791", tmp_path, "github")

    assert first["status"] == "created" and second["status"] == "reused"
    assert second["round"] == first["round"]


def test_a_rereview_on_the_same_commit_opens_a_new_round(tmp_path, monkeypatch) -> None:
    """A review rerun on the same commit: the anchor is unchanged, the comments are new.

    Comparing only the anchor would judge it "already fetched", and this round would never be dispositioned by
    anyone.
    """

    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")])
    first = lane.fetch("791", tmp_path, "github")

    _fake_gh(
        monkeypatch,
        [
            _comment(2, ANCHOR, "2026-09-17T02:00:00Z"),
            _comment(9, ANCHOR, "2026-09-17T03:00:00Z", body="重跑后新提的"),
        ],
    )
    second = lane.fetch("791", tmp_path, "github")

    assert first["round"] == 1
    assert second["status"] == "created" and second["round"] == 2


def test_a_reresolved_line_does_not_open_a_duplicate_round(tmp_path, monkeypatch) -> None:
    """After a push, GitHub re-resolves an inline comment's `line` against the new HEAD.

    That is not a content change: id, body, and `updated_at` are all untouched; the same finding just lands on a
    different line in the new tree. If the digest drifted along with it, a round of comments that had **already
    been adjudicated and replied to** would be reopened as a new round, and address-review-comments would then
    disposition the same findings again and post the ledger again.
    Observed: after one push, the `line` of this PR's two comments drifted from 44/212 to 47/223.
    """

    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z", line=44, original_line=44)])
    first = lane.fetch("791", tmp_path, "github")

    # Same comment; after the push `line` is re-resolved, while `original_line`, like the anchor, stays put.
    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z", line=47, original_line=44)])
    second = lane.fetch("791", tmp_path, "github")

    assert first["status"] == "created"
    assert second["status"] == "reused", "line-number drift is not a new round"
    assert second["round"] == first["round"]


def test_source_md_cites_the_line_in_the_anchored_commit(tmp_path, monkeypatch) -> None:
    """The location must come from the same tree as the anchor, otherwise the heading line points to a location
    that does not exist at the anchor."""

    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z", line=47, original_line=44, path="x.py")])

    result = lane.fetch("791", tmp_path, "github")
    source = Path(result["source"]).read_text(encoding="utf-8")

    assert "`x.py:44`" in source
    assert "x.py:47" not in source


@pytest.mark.parametrize("bad", ["../../outside", "a/b", "..", "/abs", "x\\y", ""])
def test_a_traversing_lane_name_is_refused(tmp_path, monkeypatch, bad) -> None:
    """The lane name is spliced directly into the artifact path, so traversal is refused -- not sanitized, since
    sanitizing would make the passed-in name diverge from the on-disk name."""

    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")])

    with pytest.raises(LaneError, match="lane 名"):
        lane.fetch("791", tmp_path, bad)
    assert not (tmp_path / "temp").exists() or not list((tmp_path / "temp").rglob("round-*")), (
        "a refused lane must not leave any on-disk artifact"
    )


def test_the_default_lane_name_still_works(tmp_path, monkeypatch) -> None:
    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")])

    result = lane.fetch("791", tmp_path, "github")

    assert result["status"] == "created"
    assert "/github/round-01" in result["roundDir"].replace("\\", "/")


def test_lane_dir_uses_the_local_branch_not_the_pr_head_ref(tmp_path, monkeypatch) -> None:
    """The artifact is used by address-review-comments' lane detection, which builds directories by local branch.

    With a fork checkout or a locally renamed branch, headRefName differs from the local branch; writing into a
    headRefName directory means writing somewhere nobody will glob, and this lane ceases to exist.
    """

    _fake_gh(
        monkeypatch,
        [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")],
        pr=_pr() | {"headRefName": "contributor-fork-branch"},
    )
    _patch_everywhere(monkeypatch, "local_branch", lambda repo: "feat/local-name")

    result = lane.fetch("791", tmp_path, "github")

    assert "/feat-local-name/github/" in result["roundDir"].replace("\\", "/")
    assert "contributor-fork-branch" not in result["roundDir"]


def test_detached_head_is_refused_not_silently_renamed(tmp_path, monkeypatch) -> None:
    """A detached HEAD has no lane identity; it must not silently fall back to headRefName.

    `local_branch` is tested directly, not via fetch: the autouse fixture is stubbing it out.
    """

    monkeypatch.setattr(lane.subprocess, "run", lambda *a, **k: type("R", (), {"returncode": 0, "stdout": ""})())

    with pytest.raises(LaneError, match="本地分支"):
        REAL_LOCAL_BRANCH(tmp_path)


def test_a_clean_rereview_at_head_is_not_reported_as_unreviewed(tmp_path, monkeypatch) -> None:
    """A reviewer can give a clean rereview on the current HEAD without any inline comments.

    At that point the inline comment group still sits on the old anchor. Looking only at `rounds` would judge
    "the reviewer has not seen this tree" and STOP, when it clearly has seen it and simply has no findings -- a
    clean rereview would then never be able to move merge-pr forward.
    """

    _fake_gh(
        monkeypatch,
        [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")],
        reviews=[
            {
                "author": {"login": "copilot"},
                "state": "APPROVED",
                "submittedAt": "2026-09-17T05:00:00Z",
                "commit": {"oid": HEAD},
            }
        ],
    )

    result = lane.fetch("791", tmp_path, "github")

    assert result["roundIsCurrentHead"] is False, "the inline comment group really is on the old anchor"
    assert result["cleanReviewAtHead"] is True, "but there is a clean rereview on the current HEAD"


def test_no_review_at_head_is_not_a_clean_rereview(tmp_path, monkeypatch) -> None:
    """With no review event at all on the current HEAD, it must not be treated as a clean rereview -- nobody has
    really looked at it."""

    _fake_gh(
        monkeypatch,
        [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")],
        reviews=[
            {
                "author": {"login": "copilot"},
                "state": "COMMENTED",
                "submittedAt": "2026-09-17T02:00:00Z",
                "commit": {"oid": ANCHOR},
            }
        ],
    )

    result = lane.fetch("791", tmp_path, "github")

    assert result["roundIsCurrentHead"] is False
    assert result["cleanReviewAtHead"] is False


def test_review_md_alone_does_not_mean_the_round_was_triaged(tmp_path, monkeypatch) -> None:
    """`review.md` is written at transcription time, which happens **before** verification, adjudication, fixing,
    and landing.

    If the run STOPs midway on a FLAG or crashes, review.md is still there. Using it as the completion criterion
    would let a round of unadjudicated comments be treated as dispositioned and merged.
    """

    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")])
    first = lane.fetch("791", tmp_path, "github")
    Path(first["roundDir"], "review.md").write_text("转写完了但还没裁决", encoding="utf-8")

    second = lane.fetch("791", tmp_path, "github")

    assert second["reviewExists"] is True
    assert second["triaged"] is False, "transcribed only is not the same as dispositioned"


def test_the_triaged_marker_is_written_only_after_a_successful_post(tmp_path, monkeypatch) -> None:
    """A round that could not be posted is not finished; it must not leave behind a file claiming it is."""

    round_dir = tmp_path / "round-01"
    round_dir.mkdir()
    triage = tmp_path / "triage.md"

    # Post succeeds -> write the marker
    _fake_gh(monkeypatch, [], posted=[])
    triage.write_text("**模式**：pr\n\n干净台账\n", encoding="utf-8")
    result = lane.post_triage("791", triage, round_dir)
    assert (round_dir / lane.TRIAGED_MARKER).is_file()
    assert result["triagedMarker"]

    # Post fails (credential hit) -> must not leave a marker
    (round_dir / lane.TRIAGED_MARKER).unlink()
    triage.write_text("**模式**：pr\n\nghp_" + "A" * 36 + "\n", encoding="utf-8")
    with pytest.raises(LaneError):
        lane.post_triage("791", triage, round_dir)
    assert not (round_dir / lane.TRIAGED_MARKER).exists()


def test_an_edit_on_an_older_anchor_is_not_ignored(tmp_path, monkeypatch) -> None:
    """The round must be selected by **latest activity**, not by key insertion order.

    `setdefault` puts an anchor at the end only the first time it appears; appending or editing comments on the
    same anchor afterwards does not move it back. So `list(rounds)[-1]` would point at an anchor that appeared
    earlier, and new activity on the old anchor would be silently ignored -- and `content_digest` includes
    `updated_at` precisely to recognize edited comments, logic that would never even get to run.
    """

    older = _comment(1, ANCHOR, "2026-09-17T01:00:00Z")
    older["updated_at"] = "2026-09-17T09:00:00Z"  # edited, latest activity
    newer = _comment(2, "d" * 40, "2026-09-17T02:00:00Z")
    _fake_gh(monkeypatch, [older, newer])

    result = lane.fetch("791", tmp_path, "github")

    assert result["anchor"] == ANCHOR, "should pick the group with the latest activity, not the last by insertion order"
    assert result["commentIds"] == [1]


def test_latest_anchor_still_picks_the_newest_round_normally(tmp_path, monkeypatch) -> None:
    """In the regular case (each round has its own anchor, increasing timestamps) selection is unchanged."""

    _fake_gh(
        monkeypatch,
        [
            _comment(1, ANCHOR, "2026-09-17T01:00:00Z"),
            _comment(2, "d" * 40, "2026-09-17T02:00:00Z"),
        ],
    )

    result = lane.fetch("791", tmp_path, "github")

    assert result["anchor"] == "d" * 40
    assert result["commentIds"] == [2]


@pytest.mark.parametrize("content", ["", "{", '{"pr": "791"}', "null", "[]"])
def test_an_unusable_marker_is_not_completion_evidence(tmp_path, monkeypatch, content) -> None:
    """The marker must prove itself by content: an empty file, truncated JSON, or an object missing `comment`
    does not count as finished.

    Looking only at `is_file()` would treat a half-written file as completion evidence, and that round's
    comments would from then on be subtracted from the delta.
    """

    _fake_gh(monkeypatch, [_comment(1, ANCHOR, "2026-09-17T01:00:00Z")])
    r1 = lane.fetch("791", tmp_path, "github")
    Path(r1["roundDir"], lane.TRIAGED_MARKER).write_text(content, encoding="utf-8")

    assert lane.round_is_triaged(Path(r1["roundDir"])) is False
    assert lane.fetch("791", tmp_path, "github")["triaged"] is False


def test_the_marker_is_written_atomically(tmp_path, monkeypatch) -> None:
    """Write a temp file first and then `os.replace`, leaving neither a half-written file nor a temp file."""

    round_dir = tmp_path / "round-01"
    round_dir.mkdir()
    triage = tmp_path / "triage.md"
    triage.write_text("**模式**：pr\n\n干净台账\n", encoding="utf-8")
    _fake_gh(monkeypatch, [], posted=[])

    lane.post_triage("791", triage, round_dir)

    assert lane.round_is_triaged(round_dir) is True
    assert not list(round_dir.glob("*.tmp")), "the temp file must already have been replaced"


def test_round_anchored_at_an_older_head_is_reported(tmp_path, monkeypatch) -> None:
    """Commits were pushed after the review ran -- no reviewer has seen the tree about to be merged."""

    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")])

    assert lane.fetch("791", tmp_path, "github")["roundIsCurrentHead"] is False


def test_round_anchored_at_current_head_is_reported(tmp_path, monkeypatch) -> None:
    _fake_gh(monkeypatch, [_comment(2, HEAD, "2026-09-17T02:00:00Z")])

    assert lane.fetch("791", tmp_path, "github")["roundIsCurrentHead"] is True


def test_no_comments_is_not_an_error(tmp_path, monkeypatch) -> None:
    _fake_gh(monkeypatch, [])

    result = lane.fetch("791", tmp_path, "github")

    assert result["status"] == "no-comments" and result["roundCount"] == 0


def test_round_identity_tracks_ids_and_latest_timestamp() -> None:
    identity = round_identity(
        [
            _comment(5, ANCHOR, "2026-09-17T01:00:00Z"),
            _comment(2, ANCHOR, "2026-09-17T04:00:00Z"),
        ]
    )

    assert identity["commentIds"] == [2, 5]
    assert identity["latestAt"] == "2026-09-17T04:00:00Z"


# ---------- post-triage fail-closed ----------


@pytest.mark.parametrize(
    "bad",
    [
        "token: ghp_abcdefghijklmnop0123",
        "看 /Users/someone/work/repo/x.md",
        "url ssh://git@host:2222/a.git",
        "api_key = sk-xxxxxxxx",
    ],
)
def test_credentials_and_local_paths_block_the_post(tmp_path, monkeypatch, bad) -> None:
    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text(f"**模式**：pr\n\n{bad}\n", encoding="utf-8")

    with pytest.raises(LaneError, match="凭据|绝对路径"):
        lane.post_triage("791", triage)
    assert posted == [], "on a hit, nothing must have been posted already"


@pytest.mark.parametrize("prefix", ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"])
def test_every_github_token_prefix_is_blocked(tmp_path, monkeypatch, prefix) -> None:
    """Miss one prefix and that whole class of token can be posted in the clear on a public PR."""

    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text(f"**模式**：pr\n\n{prefix}{'A' * 36}\n", encoding="utf-8")

    with pytest.raises(LaneError, match="凭据|绝对路径"):
        lane.post_triage("791", triage)
    assert posted == []


def test_oversized_triage_is_refused_not_truncated(tmp_path, monkeypatch) -> None:
    """What gets truncated is exactly the trailing REJECT/FLAG, i.e. the part that most needs a record."""

    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text("好" * 70000, encoding="utf-8")

    with pytest.raises(LaneError, match="上限"):
        lane.post_triage("791", triage)
    assert posted == []


def test_empty_or_missing_triage_is_refused(tmp_path, monkeypatch) -> None:
    _fake_gh(monkeypatch, [])
    missing = tmp_path / "nope.md"
    empty = tmp_path / "empty.md"
    empty.write_text("   \n", encoding="utf-8")

    with pytest.raises(LaneError, match="不存在"):
        lane.post_triage("791", missing)
    with pytest.raises(LaneError, match="空的"):
        lane.post_triage("791", empty)


def test_a_clean_triage_is_posted_verbatim(tmp_path, monkeypatch) -> None:
    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text("**模式**：pr\n\n## APPLY\n\n- C1 ...\n", encoding="utf-8")

    result = lane.post_triage("791", triage)

    assert result["comment"].endswith("issuecomment-1")
    assert posted and posted[0][:3] == ("pr", "comment", "791")
    assert "--body-file" in posted[0]


def test_gh_failure_is_reported_not_swallowed(monkeypatch, tmp_path) -> None:
    def boom(*args):
        raise LaneError("gh 不可用")

    _patch_everywhere(monkeypatch, "_gh", boom)

    with pytest.raises(LaneError):
        lane.fetch("791", tmp_path, "github")


# ---------- Contracts added after this round of review raised them ----------


def test_base_revision_is_persisted(tmp_path, monkeypatch) -> None:
    """The artifact's `**基线**` must be `<ref> @ <short-sha>`, which cannot be written if only refName is
    persisted."""

    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")])
    result = lane.fetch("791", tmp_path, "github")

    assert result["baseRef"] == "origin/master" and result["baseSha"] == "b" * 40
    recorded = json.loads((Path(result["roundDir"]) / "source.json").read_text(encoding="utf-8"))
    assert recorded["baseRef"] == "origin/master" and recorded["baseSha"] == "b" * 40


def test_a_rerun_round_renders_only_the_new_comments(tmp_path, monkeypatch) -> None:
    """A rerun on the same commit: the new round contains only the comments added this time.

    Rendering the whole group would re-disposition and re-post every previously adjudicated finding -- the
    opposite of "disposition only the latest round".
    """

    _fake_gh(monkeypatch, [_comment(2, ANCHOR, "2026-09-17T02:00:00Z", body="第一轮就提的")])
    first = lane.fetch("791", tmp_path, "github")
    # This case's premise is exactly the docstring's "previously adjudicated" -- make it concrete with the
    # disposition-complete marker. A half-finished round without the marker does not count as consumed; that has
    # its own dedicated test case.
    _mark_triaged(first["roundDir"])

    _fake_gh(
        monkeypatch,
        [
            _comment(2, ANCHOR, "2026-09-17T02:00:00Z", body="第一轮就提的"),
            _comment(9, ANCHOR, "2026-09-17T03:00:00Z", body="重跑后新提的"),
        ],
    )
    second = lane.fetch("791", tmp_path, "github")

    body = Path(second["source"]).read_text(encoding="utf-8")
    assert second["round"] == 2
    assert "重跑后新提的" in body
    assert "第一轮就提的" not in body, "previously adjudicated comments must not be replayed"


def test_an_edited_comment_opens_a_new_round(tmp_path, monkeypatch) -> None:
    """A comment is edited: the id is unchanged, the body changed. Comparing only the id would treat it as
    "already fetched"."""

    first_batch = [_comment(2, ANCHOR, "2026-09-17T02:00:00Z", body="原始正文")]
    _fake_gh(monkeypatch, first_batch)
    lane.fetch("791", tmp_path, "github")

    edited = dict(first_batch[0], body="编辑之后的正文", updated_at="2026-09-17T05:00:00Z")
    _fake_gh(monkeypatch, [edited])
    second = lane.fetch("791", tmp_path, "github")

    assert second["status"] == "created" and second["round"] == 2


def test_body_is_transcribed_verbatim(tmp_path, monkeypatch) -> None:
    """Claiming a verbatim dump means no strip -- evidence should not be touched before it is transcribed."""

    _fake_gh(
        monkeypatch,
        [
            _comment(2, ANCHOR, "2026-09-17T02:00:00Z", body="\n  缩进重要\n\n"),
        ],
    )

    result = lane.fetch("791", tmp_path, "github")

    assert "\n  缩进重要\n" in Path(result["source"]).read_text(encoding="utf-8")


@pytest.mark.parametrize(
    "bad",
    [
        r"见 C:\Users\dylan\work\repo\x.md",
        r"路径 \\fileserver\share\triage.md",
    ],
)
def test_windows_paths_block_the_post(tmp_path, monkeypatch, bad) -> None:
    """This repo has a Windows target, and the ledger may well have been produced there."""

    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text(f"**模式**：pr\n\n{bad}\n", encoding="utf-8")

    with pytest.raises(LaneError, match="凭据|绝对路径"):
        lane.post_triage("791", triage)
    assert posted == []


@pytest.mark.parametrize(
    "talk",
    [
        r"模式 `/Users/[^\s/]+/` 只认 Unix home 路径",
        r"补 `[A-Za-z]:\\(?:Users|home)\\` 形态",
    ],
)
def test_discussing_path_patterns_is_not_a_leak(tmp_path, monkeypatch, talk) -> None:
    r"""The ledger must be able to discuss "path scanning" itself.

    With the path segment written as `[^\s/]+`, the line of the ledger quoting that pattern is hit by the
    pattern itself, so a ledger dispositioning a path-scanning finding could never be posted. Hit on the spot
    while dogfooding this workflow.
    """

    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text(f"**模式**：pr\n\n{talk}\n", encoding="utf-8")

    result = lane.post_triage("791", triage)

    assert result["comment"].endswith("issuecomment-1")
    assert posted, "discussing the pattern itself should not be treated as a leak"


# ---------- Blocking absolute paths as a whole class ----------


@pytest.mark.parametrize(
    "bad",
    [
        "/tmp/triage.md",
        "/private/var/folders/zs/abc/T/run/x.json",
        "/opt/homebrew/bin/python3",
        "/Volumes/Data/stuff/y.md",
        r"D:\work\repo\x.md",
    ],
)
def test_absolute_paths_outside_home_are_blocked(tmp_path, monkeypatch, bad) -> None:
    """Enumerating by prefix inevitably misses some: pytest's tmp path is `/private/var/...`, not under any
    user directory."""

    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text(f"**模式**：pr\n\n见 {bad}\n", encoding="utf-8")

    with pytest.raises(LaneError, match="凭据|绝对路径"):
        lane.post_triage("791", triage)
    assert posted == []


@pytest.mark.parametrize(
    "bad",
    [
        "~/work/libtv-desktop/temp/triage.md",
        "~/.claude/settings.json",
    ],
)
def test_home_relative_paths_are_blocked(tmp_path, monkeypatch, bad) -> None:
    """`~/…` needs its own rule; the absolute-path rule cannot be counted on to catch it in passing.

    That rule's negative lookbehind explicitly lists `~`, so the `/repo/triage.md` segment of `~/repo/triage.md`
    can never match -- yet it still posts the local directory structure to a public PR.
    """

    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text(f"**模式**：pr\n\n见 {bad}\n", encoding="utf-8")

    with pytest.raises(LaneError, match="凭据|绝对路径"):
        lane.post_triage("791", triage)
    assert posted == []


@pytest.mark.parametrize(
    "bad",
    [
        "file:///Users/alice/secret.md",
        "file:///tmp/run/x.json",
    ],
)
def test_local_file_urls_are_blocked(tmp_path, monkeypatch, bad) -> None:
    """What follows `file://` is a local path, and the absolute-path rule's lookbehind contains `/`:

    in `file:///Users/alice/x`, `/Users` is preceded by exactly `/`, so the whole thing can never match, and the
    full local path would be posted verbatim to a public PR. http(s) are remote addresses and not covered here.
    """

    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text(f"**模式**：pr\n\n见 {bad}\n", encoding="utf-8")

    with pytest.raises(LaneError, match="凭据|绝对路径"):
        lane.post_triage("791", triage)
    assert posted == []


def test_an_untriaged_round_is_not_treated_as_consumed(tmp_path, monkeypatch) -> None:
    """A round existing does not mean it finished.

    Transcription happens before verification, adjudication, and landing; a STOP midway on a FLAG or a crash
    both leave a round directory behind. If such a half-finished round were counted as consumed, then when new
    comments arrive on the same anchor, the findings in that half-finished round would be subtracted from the
    delta -- never adjudicated, and never again appearing in any pending round.
    """

    first = _comment(1, ANCHOR, "2026-09-17T01:00:00Z", body="第一轮的意见")
    _fake_gh(monkeypatch, [first])
    r1 = lane.fetch("791", tmp_path, "github")
    Path(r1["roundDir"], "review.md").write_text("转写了但没走完", encoding="utf-8")
    assert r1["triaged"] is False

    second = _comment(2, ANCHOR, "2026-09-17T02:00:00Z", body="第二轮的新意见")
    _fake_gh(monkeypatch, [first, second])
    r2 = lane.fetch("791", tmp_path, "github")

    source = Path(r2["source"]).read_text(encoding="utf-8")
    assert "第一轮的意见" in source, "the unadjudicated one must keep being presented"
    assert "第二轮的新意见" in source


def test_a_triaged_round_is_not_re_presented(tmp_path, monkeypatch) -> None:
    """Only a dispositioned round counts as consumed -- otherwise every round would re-post the adjudicated
    findings."""

    first = _comment(1, ANCHOR, "2026-09-17T01:00:00Z", body="第一轮的意见")
    _fake_gh(monkeypatch, [first])
    r1 = lane.fetch("791", tmp_path, "github")
    _mark_triaged(r1["roundDir"])

    second = _comment(2, ANCHOR, "2026-09-17T02:00:00Z", body="第二轮的新意见")
    _fake_gh(monkeypatch, [first, second])
    r2 = lane.fetch("791", tmp_path, "github")

    source = Path(r2["source"]).read_text(encoding="utf-8")
    assert "第一轮的意见" not in source
    assert "第二轮的新意见" in source


@pytest.mark.parametrize(
    "ok",
    [
        "https://github.com/liblib-ai/libtv-desktop/pull/836#issuecomment-1",
        "`skills/address-review-comments/scripts/github_review_lane.py:36`",
        "temp/review-pr/chore-x/github/round-02/review.md",
        "四类分段 APPLY/REJECT/FLAG/HOUSEKEEPING",
    ],
)
def test_urls_and_repo_relative_paths_still_post(tmp_path, monkeypatch, ok) -> None:
    """A ledger normally contains URLs and repo-relative paths; treating them as leaks would mean the ledger
    could never be posted."""

    posted: list = []
    _fake_gh(monkeypatch, [], posted=posted)
    triage = tmp_path / "triage.md"
    triage.write_text(f"**模式**：pr\n\n{ok}\n", encoding="utf-8")

    assert lane.post_triage("791", triage)["comment"].endswith("issuecomment-1")
    assert posted


# ---------- The review-body path ----------


def test_a_body_only_review_opens_a_round(tmp_path, monkeypatch) -> None:
    """A round whose findings live only in the review body, with not a single inline comment, must be opened as
    a round.

    Observed shape: `### 🟡 Changes recommended` + `Suppressed comments (1)` +
    `Comments generated: 0`. Reading only inline comments, this round has zero entries on the comments endpoint,
    looking exactly like "seen with no findings", so it gets waved through as a clean rereview -- when it
    actually carries a concrete finding.
    """

    _fake_gh(monkeypatch, [], bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", SUPPRESSED_BODY)])

    result = lane.fetch("791", tmp_path, "github")

    assert result["status"] == "created"
    assert result["anchor"] == HEAD and result["roundIsCurrentHead"] is True
    assert result["commentCount"] == 0 and result["reviewCount"] == 1
    body = Path(result["source"]).read_text(encoding="utf-8")
    assert "Suppressed comments (1)" in body, "the body must be persisted verbatim for transcription"
    assert "wire_test.cpp:207" in body


@pytest.mark.parametrize("verdict_body", [CLOSER_LOOK_BODY, CHANGES_BODY])
def test_a_non_approving_verdict_is_not_a_clean_review(tmp_path, monkeypatch, verdict_body) -> None:
    """Neither 🔵 nor 🟡 is ready: they mean "there is more to say about this tree", not "seen with no
    findings"."""

    _fake_gh(
        monkeypatch,
        [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")],
        reviews=[
            {
                "author": {"login": "copilot"},
                "state": "COMMENTED",
                "submittedAt": "2026-09-17T05:00:00Z",
                "commit": {"oid": HEAD},
            }
        ],
        bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", verdict_body)],
    )

    result = lane.fetch("791", tmp_path, "github")

    assert result["anchor"] == HEAD, "the pending body is itself a round on the current HEAD"
    assert result["cleanReviewAtHead"] is False
    assert result["reviewCount"] == 1


def test_an_approving_verdict_stays_a_clean_review(tmp_path, monkeypatch) -> None:
    """🟢 with no findings section in the body is still a clean rereview -- otherwise every clean rereview would
    open an empty pending round."""

    _fake_gh(
        monkeypatch,
        [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")],
        reviews=[
            {
                "author": {"login": "copilot"},
                "state": "COMMENTED",
                "submittedAt": "2026-09-17T05:00:00Z",
                "commit": {"oid": HEAD},
            }
        ],
        bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", APPROVED_BODY)],
    )

    result = lane.fetch("791", tmp_path, "github")

    assert result["anchor"] == ANCHOR, "a clean body does not make a round"
    assert result["cleanReviewAtHead"] is True


def test_an_approval_that_still_carries_findings_is_outstanding() -> None:
    """When the verdict says approve but the body still carries a findings section, the findings win -- they
    cannot be read anywhere else."""

    approved_with_findings = APPROVED_BODY + "\n\n### Suppressed comments (2)\n\n**a.py:1**"
    assert (
        lane.classify_review(_review_body(11, HEAD, "2026-09-17T05:00:00Z", approved_with_findings, state="APPROVED"))[
            "outstanding"
        ]
        is True
    )


@pytest.mark.parametrize(
    "body",
    [
        "### 🟣 Something entirely new\n\n正文",  # a changed marker
        "这次评审没有标题行，直接写了意见。",  # no verdict line
        "### 审查完成\n\n请再看一眼取源那段。",  # a Chinese verdict line
    ],
)
def test_an_unrecognized_verdict_is_treated_as_outstanding(body) -> None:
    """An unrecognizable verdict line is always pending: the markers belong to an external bot, and treating the
    unknown as clean would silently skip an entire review round."""

    assert lane.classify_review(_review_body(11, HEAD, "2026-09-17T05:00:00Z", body))["outstanding"] is True


@pytest.mark.parametrize(
    "review",
    [
        {
            "id": 11,
            "user": {"login": "Copilot"},
            "state": "COMMENTED",
            "commit_id": HEAD,
            "submitted_at": "2026-09-17T05:00:00Z",
            "body": "",
        },
        {
            "id": 12,
            "user": {"login": "Copilot"},
            "state": "DISMISSED",
            "commit_id": HEAD,
            "submitted_at": "2026-09-17T05:00:00Z",
            "body": CHANGES_BODY,
        },
    ],
)
def test_an_empty_or_dismissed_review_body_is_not_a_round(review) -> None:
    """An empty body is just the envelope of inline comments; `DISMISSED` is a withdrawn opinion. Neither makes a
    round."""

    assert lane.classify_review(review)["outstanding"] is False


def test_an_edited_review_body_opens_a_new_round(tmp_path, monkeypatch) -> None:
    """When the body is edited, `submitted_at` does not change at all, so identity can only be recognized by the
    body itself."""

    _fake_gh(monkeypatch, [], bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", CHANGES_BODY)])
    first = lane.fetch("791", tmp_path, "github")
    _mark_triaged(first["roundDir"])

    _fake_gh(
        monkeypatch, [], bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", CHANGES_BODY + "\n\n又想到一条。")]
    )
    second = lane.fetch("791", tmp_path, "github")

    assert second["status"] == "created" and second["round"] == 2


def test_a_triaged_body_only_round_is_not_re_presented(tmp_path, monkeypatch) -> None:
    """A dispositioned body is not reopened: `reviewIds` must count toward the delta just like `commentIds`."""

    _fake_gh(monkeypatch, [], bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", CHANGES_BODY)])
    first = lane.fetch("791", tmp_path, "github")
    _mark_triaged(first["roundDir"])

    _fake_gh(
        monkeypatch,
        [],
        bodies=[
            _review_body(11, HEAD, "2026-09-17T05:00:00Z", CHANGES_BODY),
            _review_body(12, HEAD, "2026-09-17T06:00:00Z", SUPPRESSED_BODY),
        ],
    )
    second = lane.fetch("791", tmp_path, "github")

    assert second["status"] == "created" and second["reviewIds"] == [12], (
        "only the new body is rendered; the previously adjudicated one is not reopened"
    )


def test_the_digest_of_a_comment_only_round_is_unchanged() -> None:
    """Without review bodies the digest must be byte-for-byte identical to the old implementation.

    If it changed, the moment the upgrade lands every dispositioned round of comments on disk would, because the
    digest no longer matches, be reopened as a round, re-adjudicated, and have its ledger re-posted -- exactly
    what "the same set of comments is always the same round" is meant to prevent.
    """

    comments = [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")]
    assert lane.content_digest(comments) == "6d03537386ec7f4ffa5613821931263424930277c5173d2e677aa5690c126232"
    assert lane.content_digest(comments, []) == lane.content_digest(comments)


def test_a_review_body_and_its_inline_comments_are_one_round(tmp_path, monkeypatch) -> None:
    """Both paths of the same review merge into the same anchor group, otherwise the same review would be
    dispositioned twice."""

    _fake_gh(
        monkeypatch,
        [_comment(2, HEAD, "2026-09-17T05:00:00Z")],
        bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", SUPPRESSED_BODY)],
    )

    result = lane.fetch("791", tmp_path, "github")

    assert result["commentCount"] == 1 and result["reviewCount"] == 1
    assert result["anchor"] == HEAD


# ---------- Regressions for verdict lines and deltas ----------

# Observed `ccr-overview-v2` layout: the verdict is not the first heading but one level below the title.
V2_BODY = "<!-- ccr-overview-v2 -->\n\n## Copilot review overview\n\n### {verdict}\n\nUnresolved issues remain."


@pytest.mark.parametrize(
    "heading",
    [
        "Disapproval recommended",
        "Not approval recommended",
        "🟢 Changes recommended",
    ],
)
def test_a_heading_that_merely_contains_a_clean_token_is_not_clean(heading) -> None:
    """A clean verdict must match the **whole line**.

    Written as "contains", all three of these lines would be judged clean -- anything happening to carry a
    clean word gets waved through, fail-closed becomes decoration, and the consequence of waving through is an
    entire round of non-approving review being silently skipped.
    """

    body = f"### {heading}\n\n正文"
    assert lane.review_is_clean(body) is False
    assert lane.classify_review(_review_body(11, HEAD, "2026-09-17T05:00:00Z", body))["outstanding"] is True


def test_the_verdict_is_read_below_a_title_heading(tmp_path, monkeypatch) -> None:
    """The verdict line is not necessarily the first heading; looking only at the first would read every v2 body
    as its title line.

    The consequence cuts both ways: 🟡 is not recognized as a verdict (fail-closed can still catch that here),
    and 🟢 is not recognized either, so every clean rereview opens a phantom pending round.
    """

    changes = V2_BODY.format(verdict="🟡 Changes recommended")
    clean = V2_BODY.format(verdict="🟢 Approval recommended")

    assert lane.review_headline(changes) == "🟡 Changes recommended"
    assert lane.review_is_clean(changes) is False
    assert lane.review_headline(clean) == "🟢 Approval recommended"
    assert lane.review_is_clean(clean) is True

    _fake_gh(
        monkeypatch,
        [_comment(2, ANCHOR, "2026-09-17T02:00:00Z")],
        reviews=[
            {
                "author": {"login": "copilot"},
                "state": "COMMENTED",
                "submittedAt": "2026-09-17T05:00:00Z",
                "commit": {"oid": HEAD},
            }
        ],
        bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", clean)],
    )
    result = lane.fetch("791", tmp_path, "github")
    assert result["cleanReviewAtHead"] is True, "a clean rereview in the v2 layout must still be recognized"


def test_a_removed_review_body_does_not_re_present_triaged_comments(tmp_path, monkeypatch) -> None:
    """After a body is withdrawn the digest changes, but that is not "something new"; it is "something less".

    Falling back to reopening the whole group would re-disposition, unchanged, comments that were already
    adjudicated and whose ledger was already posted.
    """

    comments = [_comment(1, HEAD, "2026-09-17T02:00:00Z"), _comment(2, HEAD, "2026-09-17T02:00:01Z")]
    _fake_gh(monkeypatch, comments, bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", CHANGES_BODY)])
    first = lane.fetch("791", tmp_path, "github")
    _mark_triaged(first["roundDir"])

    _fake_gh(monkeypatch, comments, bodies=[])
    second = lane.fetch("791", tmp_path, "github")

    assert second["status"] == "reused", "nothing new means this round is finished"
    assert second["round"] == first["round"]
    assert second["triaged"] is True


def test_an_edited_review_is_not_dropped_when_new_items_arrive(tmp_path, monkeypatch) -> None:
    """When the delta is judged by id, a rewritten item is subtracted because its id was seen, and its new
    content never enters any round.

    Only when **both paths are empty** does it fall back to the whole group and catch it; as soon as a new
    comment arrives at the same time, the fallback does not trigger.
    """

    _fake_gh(
        monkeypatch,
        [_comment(1, HEAD, "2026-09-17T02:00:00Z")],
        bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", CHANGES_BODY)],
    )
    first = lane.fetch("791", tmp_path, "github")
    _mark_triaged(first["roundDir"])

    _fake_gh(
        monkeypatch,
        [_comment(1, HEAD, "2026-09-17T02:00:00Z"), _comment(2, HEAD, "2026-09-17T06:00:00Z")],
        bodies=[_review_body(11, HEAD, "2026-09-17T05:00:00Z", CHANGES_BODY + "\n\n又想到一条。")],
    )
    second = lane.fetch("791", tmp_path, "github")

    assert second["status"] == "created"
    assert second["commentIds"] == [2], "the unchanged 1 is not reopened"
    assert second["reviewIds"] == [11], "the edited body must enter this round"


def test_a_legacy_round_without_item_digests_still_counts_as_consumed(tmp_path, monkeypatch) -> None:
    """Rounds persisted before this field existed have only ids. They must still be recognized by id.

    Otherwise, the moment the upgrade lands, every dispositioned item on disk would be treated as new because
    "no digest found", and the whole batch would be re-dispositioned with the ledger re-posted.
    """

    _fake_gh(monkeypatch, [_comment(1, HEAD, "2026-09-17T02:00:00Z")], bodies=[])
    first = lane.fetch("791", tmp_path, "github")
    source = Path(first["roundDir"]) / "source.json"
    recorded = json.loads(source.read_text(encoding="utf-8"))
    recorded.pop("itemDigests")
    source.write_text(json.dumps(recorded, ensure_ascii=False), encoding="utf-8")
    _mark_triaged(first["roundDir"])

    _fake_gh(
        monkeypatch, [_comment(1, HEAD, "2026-09-17T02:00:00Z"), _comment(2, HEAD, "2026-09-17T06:00:00Z")], bodies=[]
    )
    second = lane.fetch("791", tmp_path, "github")

    assert second["commentIds"] == [2], "1 from the old round still counts as dispositioned"


# ---------- Boundaries of verdict markers ----------


@pytest.mark.parametrize("fence", ["```", "~~~"])
def test_an_approval_marker_inside_a_code_block_is_not_a_verdict(fence) -> None:
    """`### 🟢 Approval recommended` inside a code block is a quoted literal, not this review's verdict.

    Reading it as a heading would judge a review whose verdict is 🟡 as clean and silently skip the whole
    round's findings. And this is exactly the body shape most likely to appear when a review discusses the
    verdict markers themselves -- for example while reviewing this lane's own code.
    """

    body = (
        f"### 🟡 Changes recommended\n\n举例说明什么算干净：\n\n"
        f"{fence}\n### 🟢 Approval recommended\n{fence}\n\n正文继续。"
    )

    assert lane.review_headings(body) == ["🟡 Changes recommended"]
    assert lane.review_is_clean(body) is False
    assert lane.classify_review(_review_body(11, HEAD, "2026-09-17T05:00:00Z", body))["outstanding"] is True


def test_a_real_verdict_after_a_code_block_is_still_read() -> None:
    """Skipping fences must not also swallow the real heading after the fence."""

    body = "```\ncode\n```\n\n### 🟢 Approval recommended\n\n没问题。"

    assert lane.review_headings(body) == ["🟢 Approval recommended"]
    assert lane.review_is_clean(body) is True


def test_an_empty_changes_requested_review_is_outstanding() -> None:
    """`CHANGES_REQUESTED` is itself a blocking conclusion; an empty body does not mean no findings.

    The findings may all be in inline comments. Filtering it out as "empty body" would make a review that
    explicitly requests changes (a state only human reviewers can give) vanish entirely on this path.
    """

    review = {
        "id": 11,
        "user": {"login": "human"},
        "state": "CHANGES_REQUESTED",
        "commit_id": HEAD,
        "submitted_at": "2026-09-17T05:00:00Z",
        "body": "",
    }

    assert lane.classify_review(review)["outstanding"] is True


def test_an_empty_dismissed_review_is_still_not_outstanding() -> None:
    """Withdrawal takes precedence over everything: `DISMISSED` still does not make a round, otherwise
    withdrawal would be meaningless."""

    review = {
        "id": 11,
        "user": {"login": "human"},
        "state": "DISMISSED",
        "commit_id": HEAD,
        "submitted_at": "2026-09-17T05:00:00Z",
        "body": CHANGES_BODY,
    }

    assert lane.classify_review(review)["outstanding"] is False


# ---------- Heading parsing and state/body contradictions ----------


def test_an_approval_marker_in_an_indented_code_block_is_not_a_verdict() -> None:
    """A code block indented by 4 spaces is code as well, not a heading.

    Blocking fences but not indentation leaves the other half of the same hole open: a review quoting sample
    verdict text still gets waved through.
    """

    body = "### 🟡 Changes recommended\n\n举例：\n\n    ### 🟢 Approval recommended\n\n继续。"

    assert lane.review_headings(body) == ["🟡 Changes recommended"]
    assert lane.review_is_clean(body) is False


@pytest.mark.parametrize("line", ["#hashtag", "#123", "#!/bin/sh"])
def test_a_hash_without_following_space_is_not_a_heading(line) -> None:
    """An ATX heading's `#` must be followed by whitespace, otherwise things like `#hashtag` would also be read
    as headings."""

    assert lane.review_headings(f"{line}\n\n### 🟢 Approval recommended") == ["🟢 Approval recommended"]


def test_an_approved_state_contradicted_by_its_body_is_outstanding() -> None:
    """When the state says approve but the body says 🟡 / 🔵, the two contradict each other, and fail-closed
    classifies it as pending."""

    review = {
        "id": 11,
        "user": {"login": "x"},
        "state": "APPROVED",
        "commit_id": HEAD,
        "submitted_at": "t",
        "body": "### 🟡 Changes recommended\n\n还有问题。",
    }

    assert lane.classify_review(review)["outstanding"] is True


def test_a_human_approval_with_prose_stays_clean() -> None:
    """Conversely, an APPROVED body must not be required to contain a clean verdict.

    Those markers are machine-review vocabulary; humans commonly write an ordinary remark when approving, and
    insisting on it would turn every human approval into a pending round.
    """

    review = {
        "id": 11,
        "user": {"login": "human"},
        "state": "APPROVED",
        "commit_id": HEAD,
        "submitted_at": "t",
        "body": "LGTM, nice cleanup.",
    }

    assert lane.classify_review(review)["outstanding"] is False


def test_a_changes_requested_body_with_a_green_heading_is_not_approved(monkeypatch) -> None:
    """The merge gate must not look only at a green heading plus counts -- that would bypass the REST state."""

    _verdict_gh(
        monkeypatch,
        HEAD,
        [
            {
                "id": 11,
                "user": {"login": "x"},
                "state": "CHANGES_REQUESTED",
                "commit_id": HEAD,
                "submitted_at": "t",
                "body": "### 🟢 Approval recommended",
            }
        ],
    )

    assert lane.head_verdict("791")["approved"] is False


def test_a_state_only_approval_counts_as_approved(monkeypatch) -> None:
    """`APPROVED` can have just a state and no body -- that is exactly the most common shape of a human
    approval."""

    _verdict_gh(
        monkeypatch,
        HEAD,
        [
            {
                "id": 11,
                "user": {"login": "human"},
                "state": "APPROVED",
                "commit_id": HEAD,
                "submitted_at": "t",
                "body": "",
            }
        ],
    )

    result = lane.head_verdict("791")
    assert result["hasReviewAtHead"] is True and result["approved"] is True


def test_an_empty_commented_review_is_not_an_approval(monkeypatch) -> None:
    """A `COMMENTED` with an empty body is just the envelope of inline comments, not an approval.

    Judging only "no pending conclusion" would treat it as an approval -- approval needs a separate positive
    signal.
    """

    _verdict_gh(
        monkeypatch,
        HEAD,
        [{"id": 11, "user": {"login": "x"}, "state": "COMMENTED", "commit_id": HEAD, "submitted_at": "t", "body": ""}],
    )

    assert lane.head_verdict("791")["approved"] is False


def test_a_dismissed_review_does_not_mask_the_active_verdict(monkeypatch) -> None:
    """A `DISMISSED` with a body sorts last and masks the one that is actually in effect."""

    _verdict_gh(
        monkeypatch,
        HEAD,
        [
            {
                "id": 11,
                "user": {"login": "x"},
                "state": "CHANGES_REQUESTED",
                "commit_id": HEAD,
                "submitted_at": "2026-09-17T05:00:00Z",
                "body": "要改。",
            },
            {
                "id": 12,
                "user": {"login": "x"},
                "state": "DISMISSED",
                "commit_id": HEAD,
                "submitted_at": "2026-09-17T06:00:00Z",
                "body": "### 🟢 Approval recommended",
            },
        ],
    )

    result = lane.head_verdict("791")
    assert result["reviewId"] == 11 and result["approved"] is False


@pytest.mark.parametrize(
    "body",
    [
        "```python\n### 🟢 Approval recommended\n```\n\n### 🟡 Changes recommended",
        "````\n```\n### 🟢 Approval recommended\n````\n\n### 🟡 Changes recommended",
    ],
)
def test_a_fence_is_closed_only_by_a_matching_fence(body) -> None:
    """Storing only the first three characters and checking closure with startswith would treat lines like
    ```python, and triple backticks inside a four-backtick fence, as closing, so the green heading inside the
    code block turns back into a "real verdict"."""

    assert lane.review_headings(body) == ["🟡 Changes recommended"]
    assert lane.review_is_clean(body) is False


def test_conflicting_verdicts_are_not_clean() -> None:
    """When the body has both a real 🟡 and a 🟢 in a quoted passage, fail-closed judges it not clean."""

    assert lane.review_is_clean("### 🟡 Changes recommended\n\n### 🟢 Approval recommended") is False
    assert lane.review_is_clean("### 🟢 Approval recommended") is True


def test_an_empty_comment_review_does_not_mask_the_verdict(monkeypatch) -> None:
    """Replying to a review comment creates a new COMMENTED review with an empty body.

    It sorts last by time, so a single reply by the author masks the reviewer's real verdict (observed: verdict
    became None).
    """

    _verdict_gh(
        monkeypatch,
        HEAD,
        [
            {
                "id": 11,
                "user": {"login": "bot"},
                "state": "COMMENTED",
                "commit_id": HEAD,
                "submitted_at": "2026-09-17T05:00:00Z",
                "body": "### 🟡 Changes recommended\n\n有问题。",
            },
            {
                "id": 12,
                "user": {"login": "me"},
                "state": "COMMENTED",
                "commit_id": HEAD,
                "submitted_at": "2026-09-17T06:00:00Z",
                "body": "",
            },
        ],
    )

    result = lane.head_verdict("791")
    assert result["reviewId"] == 11
    assert result["verdict"] == "🟡 Changes recommended"


def test_more_than_six_hashes_is_not_a_heading() -> None:
    """CommonMark ATX headings have at most 6 `#`; leaving it uncapped is yet another way around fail-closed."""

    assert lane.review_headings("####### 🟢 Approval recommended") == []
    assert lane.review_is_clean("####### 🟢 Approval recommended") is False


def test_a_structured_non_approval_verdict_under_approved_is_outstanding() -> None:
    """Blocking only 🟡 / 🔵 is not enough: `🟢 Changes recommended` is a structured but non-approving
    verdict."""

    review = {
        "id": 11,
        "user": {"login": "x"},
        "state": "APPROVED",
        "commit_id": HEAD,
        "submitted_at": "t",
        "body": "### 🟢 Changes recommended\n\n还要改。",
    }

    assert lane.classify_review(review)["outstanding"] is True


def test_same_timestamp_reviews_are_ordered_by_id(monkeypatch) -> None:
    """Two reviews on the same HEAD can be submitted in the same second; sorting only by time falls back to the
    REST return order."""

    _verdict_gh(
        monkeypatch,
        HEAD,
        [
            {
                "id": 12,
                "user": {"login": "x"},
                "state": "COMMENTED",
                "commit_id": HEAD,
                "submitted_at": "2026-09-17T05:00:00Z",
                "body": "### 🟡 Changes recommended\n\n后提交的保留意见。",
            },
            {
                "id": 11,
                "user": {"login": "x"},
                "state": "COMMENTED",
                "commit_id": HEAD,
                "submitted_at": "2026-09-17T05:00:00Z",
                "body": "### 🟢 Approval recommended",
            },
        ],
    )

    result = lane.head_verdict("791")
    assert result["reviewId"] == 12 and result["approved"] is False


def test_a_verdict_only_review_reports_at_least_one_outstanding(monkeypatch) -> None:
    """With only a verdict and no findings section, `findings` is 0, but it really is pending.

    Reporting 0 would let callers read a self-contradictory state of "pending, but 0 outstanding".
    """

    _verdict_gh(
        monkeypatch,
        HEAD,
        [
            {
                "id": 11,
                "user": {"login": "x"},
                "state": "COMMENTED",
                "commit_id": HEAD,
                "submitted_at": "t",
                "body": "### 🔵 Needs a closer look\n\n要人再看一眼。",
            }
        ],
    )

    result = lane.head_verdict("791")
    assert result["outstanding"] is True and result["outstandingFindings"] >= 1


def test_a_legacy_item_edited_in_place_is_re_presented(tmp_path, monkeypatch) -> None:
    """Legacy rounds have only ids: when content under the same id was edited, recognizing by id would make the
    rewritten finding disappear forever."""

    _fake_gh(monkeypatch, [_comment(1, HEAD, "2026-09-17T02:00:00Z", body="原文")], bodies=[])
    first = lane.fetch("791", tmp_path, "github")
    source = Path(first["roundDir"]) / "source.json"
    recorded = json.loads(source.read_text(encoding="utf-8"))
    recorded.pop("itemDigests")
    source.write_text(json.dumps(recorded, ensure_ascii=False), encoding="utf-8")
    _mark_triaged(first["roundDir"])

    _fake_gh(monkeypatch, [_comment(1, HEAD, "2026-09-17T02:00:00Z", body="改写过的意见")], bodies=[])
    second = lane.fetch("791", tmp_path, "github")

    assert second["status"] == "created"
    assert second["commentIds"] == [1], "the rewritten one must be reopened"


def test_two_legacy_rounds_on_one_anchor_are_compared_separately(tmp_path, monkeypatch) -> None:
    """Each legacy round must be compared against **its own** digest.

    If their ids were merged into one set before comparing, then as soon as one anchor has had two rounds
    (round one id 1, round two adding id 2), the union digest can never match the last round's digest, so every
    upgrade would reopen the dispositioned findings as a whole batch.
    """

    def legacy(round_dir):
        source = Path(round_dir) / "source.json"
        recorded = json.loads(source.read_text(encoding="utf-8"))
        recorded.pop("itemDigests", None)
        source.write_text(json.dumps(recorded, ensure_ascii=False), encoding="utf-8")
        _mark_triaged(round_dir)

    c1 = _comment(1, HEAD, "2026-09-17T02:00:00Z")
    c2 = _comment(2, HEAD, "2026-09-17T03:00:00Z")
    _fake_gh(monkeypatch, [c1], bodies=[])
    legacy(lane.fetch("791", tmp_path, "github")["roundDir"])
    _fake_gh(monkeypatch, [c1, c2], bodies=[])
    legacy(lane.fetch("791", tmp_path, "github")["roundDir"])

    # Third time: neither of the two changed, so no new round should open
    _fake_gh(monkeypatch, [c1, c2, _comment(3, HEAD, "2026-09-17T04:00:00Z")], bodies=[])
    third = lane.fetch("791", tmp_path, "github")

    assert third["commentIds"] == [3], "the unchanged 1 / 2 are not reopened"


def test_our_own_thread_replies_do_not_open_new_rounds(tmp_path, monkeypatch) -> None:
    """Adjudication replies are inline comments with `in_reply_to_id`.

    Not excluding them is an infinite loop: the next fetch treats the disposition the author just posted as a
    new finding and opens the next round, which after disposition gets replied to again, never converging --
    while the rule "no merge while there are unread findings" keeps blocking.
    """

    finding = _comment(1, HEAD, "2026-09-17T02:00:00Z")
    _fake_gh(monkeypatch, [finding], bodies=[])
    first = lane.fetch("791", tmp_path, "github")
    _mark_triaged(first["roundDir"])

    our_reply = _comment(2, HEAD, "2026-09-17T03:00:00Z", body="作者裁决：REJECT …")
    our_reply["in_reply_to_id"] = 1
    _fake_gh(monkeypatch, [finding, our_reply], bodies=[])
    second = lane.fetch("791", tmp_path, "github")

    assert second["status"] == "reused", "our own replies do not constitute a new round"


def test_a_reply_is_not_sent_twice_after_a_midway_failure(tmp_path, monkeypatch) -> None:
    """`triaged.json` is written only at the round's final step, so rerunning after a midway failure would resend
    replies that already succeeded."""

    sent: list = []

    def gh(*args):
        if args[0] == "api" and "-X" in args:
            sent.append(args)
            return "https://x.invalid/899#discussion_r1\n"
        raise AssertionError(args)

    _patch_everywhere(monkeypatch, "_gh", gh)
    body = tmp_path / "d.md"
    body.write_text("作者裁决：REJECT，一手反证见台账。", encoding="utf-8")
    round_dir = tmp_path / "round-01"
    round_dir.mkdir()

    first = lane.reply_to_finding("899", "4051967357", body, round_dir)
    second = lane.reply_to_finding("899", "4051967357", body, round_dir)

    assert len(sent) == 1, "a rerun must not send again"
    assert second["skipped"] == "already-replied"
    assert second["reply"] == first["reply"]


@pytest.mark.parametrize("unknown", ["🟣 Something new", "⚫ Unclassified"])
def test_an_unknown_structured_verdict_blocks_a_clean_one(unknown) -> None:
    """Enumerating only known non-approving markers is not enough.

    An unrecognized structured verdict neither counts as a conflict nor blocks a green heading in the same body,
    so the whole round gets judged clean -- exactly the kind of thing fail-closed is meant to prevent.
    """

    body = f"### {unknown}\n\n### 🟢 Approval recommended"

    assert lane.review_is_clean(body) is False
    assert lane.classify_review(_review_body(11, HEAD, "t", body, state="APPROVED"))["outstanding"] is True


def test_a_body_without_any_verdict_heading_is_not_clean() -> None:
    """With no verdict line at all it is not clean -- clean must be **stated**, not assumed by default."""

    assert lane.review_is_clean("## Notes\n\n随手记了点东西。") is False


def test_a_clean_verdict_without_an_emoji_is_still_clean() -> None:
    """A clean verdict is allowed without an emoji.

    Collecting verdict lines only by "the first character is a symbol" would collect nothing for
    `### Approval recommended`, so it would be judged not clean -- a clean rereview would instead open a pending
    round.
    """

    assert lane.review_is_clean("### Approval recommended") is True
    assert lane.classify_review(_review_body(11, HEAD, "t", "### Approval recommended"))["outstanding"] is False


def test_a_no_emoji_clean_verdict_is_still_blocked_by_a_conflicting_one() -> None:
    """Loosening what gets collected must not loosen the conflict check."""

    assert lane.review_is_clean("### 🟡 Changes recommended\n\n### Approval recommended") is False


# ---------- The verdict the merge gate reads ----------


def _verdict_gh(monkeypatch, head, reviews):
    def gh(*args):
        if args[0] == "pr" and args[1] == "view":
            return json.dumps(
                {
                    "number": 791,
                    "headRefName": "feat/x",
                    "headRefOid": head,
                    "baseRefName": "master",
                    "isDraft": False,
                    "url": "https://x.invalid/791",
                }
            )
        if args[0] == "api":
            return json.dumps(reviews)
        raise AssertionError(args)

    _patch_everywhere(monkeypatch, "_gh", gh)


def test_a_triaged_round_does_not_make_an_unapproved_verdict_approved(monkeypatch) -> None:
    """The merge gate needs **the reviewer's own conclusion**, not our marker saying we finished dispositioning.

    After a round is dispositioned the local `triaged` is true, while the reviewer may well still say
    `🔵 Needs a closer look` plus `Open (3)`. Using triaged alone as the gate would merge anyway while the
    reviewer explicitly says there are still unresolved findings.
    """

    body = "## Copilot review overview\n\n### 🔵 Needs a closer look\n\n<summary><strong>Open (3)</strong></summary>"
    _verdict_gh(monkeypatch, HEAD, [_review_body(11, HEAD, "2026-09-17T05:00:00Z", body)])

    result = lane.head_verdict("791")

    assert result["approved"] is False
    assert result["clean"] is False
    assert result["outstandingFindings"] == 3


def test_a_clean_verdict_with_nothing_open_is_approved(monkeypatch) -> None:
    body = "## Copilot review overview\n\n### 🟢 Approval recommended\n\n未发现阻塞问题。"
    _verdict_gh(monkeypatch, HEAD, [_review_body(11, HEAD, "2026-09-17T05:00:00Z", body)])

    assert lane.head_verdict("791")["approved"] is True


def test_a_clean_verdict_that_still_lists_open_items_is_not_approved(monkeypatch) -> None:
    """A verdict that says approve while still listing unresolved findings is not an approval -- those findings
    cannot be read anywhere else."""

    body = "### 🟢 Approval recommended\n\n<summary><strong>Open (1)</strong></summary>"
    _verdict_gh(monkeypatch, HEAD, [_review_body(11, HEAD, "2026-09-17T05:00:00Z", body)])

    result = lane.head_verdict("791")
    assert result["clean"] is True and result["approved"] is False


def test_an_approval_on_an_older_commit_does_not_approve_the_current_tree(monkeypatch) -> None:
    """An approval on an older commit does not constitute approval of the tree about to be merged."""

    _verdict_gh(
        monkeypatch, HEAD, [_review_body(11, ANCHOR, "2026-09-17T05:00:00Z", "### 🟢 Approval recommended\n\n没问题。")]
    )

    result = lane.head_verdict("791")
    assert result["hasReviewAtHead"] is False
    assert result["approved"] is False


def test_the_latest_review_at_head_wins(monkeypatch) -> None:
    """When the same tree was reviewed twice, the latest one wins; an earlier approval must not mask later
    reservations."""

    _verdict_gh(
        monkeypatch,
        HEAD,
        [
            _review_body(11, HEAD, "2026-09-17T05:00:00Z", "### 🟢 Approval recommended\n\n没问题。"),
            _review_body(12, HEAD, "2026-09-17T06:00:00Z", "### 🟡 Changes recommended\n\n又看出问题了。"),
        ],
    )

    result = lane.head_verdict("791")
    assert result["reviewId"] == 12 and result["approved"] is False


# ---------- Review coverage ----------


def _reviews_gh(monkeypatch, head, reviews):
    def gh(*args):
        if args[0] == "pr" and args[1] == "view":
            return json.dumps({"headRefOid": head, "reviews": reviews})
        raise AssertionError(args)

    _patch_everywhere(monkeypatch, "_gh", gh)


def _review(login, oid, at):
    return {"author": {"login": login}, "state": "COMMENTED", "submittedAt": at, "commit": {"oid": oid}}


def test_reviewer_stuck_on_an_older_commit_is_reported_pending(monkeypatch) -> None:
    """The states "zero comments" and "has not seen this tree yet" are indistinguishable on the comments endpoint; review
    events carry a commit, so they can tell."""

    # codex reviewed two commits here, so it counts as "re-reviews"; being behind HEAD is what truly means
    # it has not gotten to it yet.
    _reviews_gh(
        monkeypatch,
        HEAD,
        [
            _review("codex", "b" * 40, "2026-09-17T02:00:00Z"),
            _review("codex", "c" * 40, "2026-09-17T03:00:00Z"),
            _review("copilot", HEAD, "2026-09-17T04:00:00Z"),
        ],
    )

    result = lane.review_coverage("836")

    assert result["pendingReviewers"] == ["codex"]
    assert result["coveredAtHead"] is False


def test_all_reviewers_at_head_is_covered(monkeypatch) -> None:
    _reviews_gh(
        monkeypatch,
        HEAD,
        [
            _review("copilot", HEAD, "2026-09-17T04:00:00Z"),
            _review("codex", HEAD, "2026-09-17T04:01:00Z"),
        ],
    )

    assert lane.review_coverage("836")["coveredAtHead"] is True


def test_a_pr_never_reviewed_is_not_reported_as_covered(monkeypatch) -> None:
    """Never reviewed by anyone is not the same as "reviewed and clean" -- the repo may not have machine review
    hooked up at all."""

    _reviews_gh(monkeypatch, HEAD, [])

    result = lane.review_coverage("836")

    assert result["everReviewed"] is False and result["coveredAtHead"] is False


def test_only_the_latest_review_per_reviewer_counts(monkeypatch) -> None:
    _reviews_gh(
        monkeypatch,
        HEAD,
        [
            _review("copilot", "c" * 40, "2026-09-17T03:00:00Z"),
            _review("copilot", HEAD, "2026-09-17T04:00:00Z"),
        ],
    )

    assert lane.review_coverage("836")["coveredAtHead"] is True


def test_a_reviewer_that_never_re_reviews_does_not_block(monkeypatch) -> None:
    """A reviewer that has reviewed only once does not count as "has not gotten to it yet".

    Observed: Copilot re-reviews on every push, while codex reviewed once when the PR was opened and skipped all
    three subsequent pushes. Requiring "all reviewers cover the current HEAD" can never hold for this
    combination, and waiting inevitably runs into the timeout -- that is not caution, it is writing the gate as
    a deadlock.
    """

    _reviews_gh(
        monkeypatch,
        HEAD,
        [
            _review("codex", "c" * 40, "2026-09-17T03:00:00Z"),
            _review("copilot", "c" * 40, "2026-09-17T03:01:00Z"),
            _review("copilot", "d" * 40, "2026-09-17T03:30:00Z"),
            _review("copilot", HEAD, "2026-09-17T04:00:00Z"),
        ],
    )

    result = lane.review_coverage("836")

    assert result["recurringReviewers"] == ["copilot"]
    assert result["pendingReviewers"] == []
    assert result["coveredAtHead"] is True
    assert result["reviewedOnceAt"] == {"codex": "c" * 12}


def test_a_recurring_reviewer_behind_head_still_blocks(monkeypatch) -> None:
    """A reviewer proven to re-review that is behind HEAD still means "has not gotten to it yet"."""

    _reviews_gh(
        monkeypatch,
        HEAD,
        [
            _review("copilot", "c" * 40, "2026-09-17T03:00:00Z"),
            _review("copilot", "d" * 40, "2026-09-17T03:30:00Z"),
        ],
    )

    result = lane.review_coverage("836")

    assert result["pendingReviewers"] == ["copilot"]
    assert result["coveredAtHead"] is False


def test_a_one_shot_reviewer_behind_head_is_not_coverage(monkeypatch) -> None:
    """The state "nobody needs to be waited for" is not the same as "somebody has seen this tree".

    When the only reviewer has reviewed just once and is stuck on an old commit, it does not count as recurring,
    so `pendingReviewers` is empty. Judging coverage only by "no pending" would mark a tree **never seen by any
    reviewer** as covered, and that is exactly the value merge-pr uses to decide whether it can merge. Both
    signals are required: there really is a review event on the current HEAD, and everyone who re-reviews has
    caught up.
    """

    _reviews_gh(
        monkeypatch,
        HEAD,
        [
            _review("codex", "c" * 40, "2026-09-17T03:00:00Z"),
        ],
    )

    result = lane.review_coverage("836")

    assert result["pendingReviewers"] == [], (
        "someone who reviewed only once should not be treated as not having gotten to it yet"
    )
    assert result["headHasReview"] is False
    assert result["coveredAtHead"] is False
    assert result["everReviewed"] is True


def test_head_has_review_is_reported_independently(monkeypatch) -> None:
    """HEAD has a review while another recurring reviewer is behind: the two signals each say their own thing."""

    _reviews_gh(
        monkeypatch,
        HEAD,
        [
            _review("codex", HEAD, "2026-09-17T04:00:00Z"),
            _review("copilot", "c" * 40, "2026-09-17T03:00:00Z"),
            _review("copilot", "d" * 40, "2026-09-17T03:30:00Z"),
        ],
    )

    result = lane.review_coverage("836")

    assert result["headHasReview"] is True
    assert result["pendingReviewers"] == ["copilot"]
    assert result["coveredAtHead"] is False


def test_a_push_during_fetch_is_reported_not_guessed(tmp_path, monkeypatch) -> None:
    """Someone pushes between fetching head and fetching comments: `roundIsCurrentHead` must not be judged from
    a stale snapshot.

    In the old order, comments came from the new state while head was a stale snapshot, so `anchor == head`
    could hold; the lane would then claim to cover the current HEAD and go on adjudicating and landing on a
    tree no reviewer had seen.
    """

    heads = [HEAD, "f" * 40]  # the HEAD read the second time has already advanced

    def gh(*args):
        if args[0] == "pr" and args[1] == "view":
            return json.dumps(_pr(heads.pop(0) if heads else "f" * 40))
        if args[0] == "api":
            return json.dumps([_comment(1, HEAD, "2026-09-17T02:00:00Z")])
        raise AssertionError(args)

    _patch_everywhere(monkeypatch, "_gh", gh)

    result = lane.fetch("836", tmp_path, "github")

    assert result["status"] == "head-moved"
    assert result["head"] == "f" * 40 and result["previousHead"] == HEAD
    assert "roundIsCurrentHead" not in result, "no coverage judgment may be given when HEAD has moved"


def test_a_stable_head_still_fetches_normally(tmp_path, monkeypatch) -> None:
    _fake_gh(monkeypatch, [_comment(1, HEAD, "2026-09-17T02:00:00Z")])

    result = lane.fetch("836", tmp_path, "github")

    assert result["status"] == "created" and result["roundIsCurrentHead"] is True
