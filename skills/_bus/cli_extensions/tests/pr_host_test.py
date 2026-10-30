"""pr_host 的机械契约：平台判定、GitHub 命令逐字不变、Codeup 翻译与 fail-closed。"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import pr_host  # noqa: E402
import pr_host_codeup  # noqa: E402
from pr_host_common import HostError, Remote  # noqa: E402


CODEUP_URL = "https://codeup.aliyun.com/663dc7b313a39cc55c4cb5b8/mobile/libtv-app.git"
HEAD = "a" * 40
OTHER = "b" * 40


@pytest.mark.parametrize(
    ("url", "host", "path"),
    [
        ("https://github.com/liblib-ai/libtv-app.git", "github.com", "liblib-ai/libtv-app"),
        ("git@github.com:liblib-ai/libtv-app.git", "github.com", "liblib-ai/libtv-app"),
        ("ssh://git@github.com/liblib-ai/libtv-app", "github.com", "liblib-ai/libtv-app"),
        (CODEUP_URL, "codeup.aliyun.com", "663dc7b313a39cc55c4cb5b8/mobile/libtv-app"),
        ("git@codeup.aliyun.com:org/mobile/libtv-app.git", "codeup.aliyun.com", "org/mobile/libtv-app"),
    ],
)
def test_parse_remote(url, host, path):
    remote = pr_host.parse_remote(url)
    assert (remote.host, remote.path) == (host, path)


def test_parse_remote_drops_credentials():
    remote = pr_host.parse_remote("https://oauth2:s3cret@codeup.aliyun.com/org/g/r.git")
    assert "s3cret" not in remote.url
    assert remote.path == "org/g/r"


def test_parse_remote_rejects_garbage():
    with pytest.raises(HostError):
        pr_host.parse_remote("not a url")


def test_detect_rejects_unknown_host(monkeypatch):
    monkeypatch.setattr(
        pr_host, "origin_remote", lambda repo=None: Remote("gitlab.com", "a/b", "https://gitlab.com/a/b")
    )
    with pytest.raises(HostError, match="不支持的代码托管平台"):
        pr_host.detect()


def test_detect_github(monkeypatch):
    monkeypatch.setattr(pr_host, "origin_remote", lambda repo=None: pr_host.parse_remote("git@github.com:o/r.git"))
    assert pr_host.detect().name == "github"


# ---------- GitHub：命令必须与迁移前 SKILL.md 写的逐字相同 ----------


@pytest.fixture
def gh_calls(monkeypatch):
    calls = []
    replies = {}

    def fake(*args):
        calls.append(list(args))
        return replies.get(args[:2], "")

    monkeypatch.setattr(pr_host, "_gh", fake)
    return calls, replies


def _github():
    return pr_host.GitHubHost(pr_host.parse_remote("https://github.com/o/r.git"))


def test_github_find(gh_calls):
    calls, replies = gh_calls
    replies[("pr", "list")] = "[]"
    assert _github().find("feat/x") == []
    assert calls == [["pr", "list", "--head", "feat/x", "--state", "open", "--json", "number,title,url,isDraft"]]


def test_github_create_is_draft_and_parses_number(gh_calls):
    calls, replies = gh_calls
    replies[("pr", "create")] = "https://github.com/o/r/pull/42\n"
    assert _github().create("feat/x", "main", "标题", "body.md") == {
        "number": 42,
        "url": "https://github.com/o/r/pull/42",
        "isDraft": True,
    }
    assert calls == [
        ["pr", "create", "--draft", "--head", "feat/x", "--base", "main", "--title", "标题", "--body-file", "body.md"]
    ]


def test_github_create_without_url_fails(gh_calls):
    _, replies = gh_calls
    replies[("pr", "create")] = "something odd"
    with pytest.raises(HostError):
        _github().create("feat/x", "main", "t", "b.md")


def test_github_edit_ready_comment(gh_calls):
    calls, _ = gh_calls
    host = _github()
    host.edit("7", "t", "b.md")
    host.ready("7")
    host.comment("7", "c.md")
    assert calls == [
        ["pr", "edit", "7", "--title", "t", "--body-file", "b.md"],
        ["pr", "ready", "7"],
        ["pr", "comment", "7", "--body-file", "c.md"],
    ]


def test_github_view_flattens_merge_commit(gh_calls):
    _, replies = gh_calls
    replies[("pr", "view")] = json.dumps({"number": 7, "mergeCommit": {"oid": HEAD}, "state": "MERGED"})
    assert _github().view("7")["mergeCommit"] == HEAD


@pytest.mark.parametrize(("admin", "extra"), [(False, []), (True, ["--admin"])])
def test_github_merge_matches_head(gh_calls, admin, extra):
    calls, replies = gh_calls
    replies[("pr", "view")] = json.dumps({"number": 7, "mergeCommit": None})
    _github().merge("7", HEAD, admin, None)
    assert calls[0] == ["pr", "merge", "7", "--squash", *extra, "--delete-branch=false", "--match-head-commit", HEAD]


def test_github_request_review_falls_back_to_bot_login(monkeypatch):
    calls = []

    def fake(*args):
        calls.append(args[-1])
        if args[-1] == "@copilot":
            raise HostError("unknown reviewer")
        return ""

    monkeypatch.setattr(pr_host, "_gh", fake)
    _github().request_review("7")
    assert calls == ["@copilot", "copilot-pull-request-reviewer"]


def test_codeup_only_commands_refuse_github(gh_calls):
    args = pr_host.build_parser().parse_args(["checks", "--pr", "7"])
    with pytest.raises(HostError, match="只用于 Codeup"):
        pr_host.run(args, _github())


def test_main_rejects_short_head(capsys):
    assert pr_host.main(["merge", "--pr", "7", "--head", "abc123"]) == 2
    assert "40" in capsys.readouterr().err


# ---------- Codeup ----------


class FakeAliyun:
    """按子命令名回放 JSON，并记录每次调用的完整 argv 与 env。"""

    def __init__(self, replies):
        self.replies = replies
        self.calls = []

    def __call__(self, cmd, **kwargs):
        self.calls.append((cmd, kwargs.get("env") or {}))
        reply = self.replies.get(cmd[2])
        if callable(reply):
            reply = reply(cmd)
        if isinstance(reply, Exception):
            return subprocess.CompletedProcess(cmd, 1, "", str(reply))
        return subprocess.CompletedProcess(cmd, 0, json.dumps(reply), "")

    def commands(self):
        return [cmd[2] for cmd, _ in self.calls]

    def args_of(self, name):
        return next(cmd for cmd, _ in self.calls if cmd[2] == name)


def _patch_sets(head=HEAD):
    return [
        {"relatedMergeItemType": "MERGE_SOURCE", "versionNo": 1, "commitId": OTHER, "patchSetBizId": "p1"},
        {"relatedMergeItemType": "MERGE_SOURCE", "versionNo": 2, "commitId": head, "patchSetBizId": "p2"},
        {"relatedMergeItemType": "MERGE_TARGET", "versionNo": 9, "commitId": "c" * 40, "patchSetBizId": "base"},
    ]


def _mr(title="[WIP] 标题", status="UNDER_REVIEW", **extra):
    return {
        "localId": 5,
        "title": title,
        "description": "正文",
        "status": status,
        "sourceBranch": "infra/x",
        "targetBranch": "main",
        "detailUrl": "https://codeup.aliyun.com/o/g/r/change/5",
        "conflictCheckStatus": "NO_CONFLICT",
        "behind": 0,
        **extra,
    }


@pytest.fixture
def codeup_env(monkeypatch):
    monkeypatch.setenv(pr_host_codeup.TOKEN_ENV, "tok-SECRET")
    monkeypatch.delenv(pr_host_codeup.ORG_ENV, raising=False)


def _codeup(replies):
    defaults = {"codeup-get-repository": {"id": 7607455}, "codeup-get-change-request": _mr(author={"id": 1})}
    fake = FakeAliyun({**defaults, **replies})
    return pr_host_codeup.CodeupHost(pr_host.parse_remote(CODEUP_URL), runner=fake), fake


def test_codeup_requires_token(monkeypatch):
    monkeypatch.delenv(pr_host_codeup.TOKEN_ENV, raising=False)
    with pytest.raises(HostError, match=pr_host_codeup.TOKEN_ENV):
        pr_host_codeup.CodeupHost(pr_host.parse_remote(CODEUP_URL))


def test_codeup_token_goes_through_env_not_argv(codeup_env):
    host, fake = _codeup({})
    host.repo_id()
    cmd, env = fake.calls[0]
    assert "tok-SECRET" not in " ".join(cmd)
    assert env[pr_host_codeup.TOKEN_ENV] == "tok-SECRET"
    # 组织 ID 缺省取仓库路径第一段；仓库路径整体 URL 编码。
    assert cmd[3:5] == ["--organization-id", "663dc7b313a39cc55c4cb5b8"]
    assert cmd[-1] == "663dc7b313a39cc55c4cb5b8%2Fmobile%2Flibtv-app"


def test_codeup_error_redacts_token(codeup_env):
    host, _ = _codeup({"codeup-get-repository": RuntimeError("bad token tok-SECRET")})
    with pytest.raises(HostError) as error:
        host.repo_id()
    assert "tok-SECRET" not in str(error.value)


def test_codeup_accepts_raw_control_characters(codeup_env):
    fake = FakeAliyun({})
    fake.replies["codeup-get-repository"] = {"id": 1}
    host = pr_host_codeup.CodeupHost(pr_host.parse_remote(CODEUP_URL), runner=fake)

    def raw(cmd, **kwargs):
        return subprocess.CompletedProcess(cmd, 0, '[{"content": "a\tb"}]', "")

    host._runner = raw
    assert host._call("codeup-list-merge-request-comments") == [{"content": "a\tb"}]


def test_codeup_create_marks_draft_with_title_prefix(codeup_env, tmp_path):
    body = tmp_path / "b.md"
    body.write_text("## 摘要\n", encoding="utf-8")
    host, fake = _codeup({"codeup-create-change-request": {"localId": 5, "detailUrl": "u"}})
    assert host.create("infra/x", "main", "标题", str(body)) == {"number": 5, "url": "u", "isDraft": True}
    args = fake.args_of("codeup-create-change-request")
    assert args[args.index("--title") + 1] == "[WIP] 标题"
    assert args[args.index("--description") + 1] == "## 摘要\n"
    for flag in ("--repository-id", "--source-project-id", "--target-project-id"):
        assert args[args.index(flag) + 1] == "7607455"


def test_codeup_find_filters_branch_and_strips_prefix(codeup_env):
    rows = [
        {"localId": 5, "sourceBranch": "infra/x", "projectId": 7607455, "title": "[WIP] 甲", "detailUrl": "u5"},
        {"localId": 6, "sourceBranch": "other", "projectId": 7607455, "title": "乙", "detailUrl": "u6"},
    ]
    host, _ = _codeup({"codeup-list-change-requests": rows})
    assert host.find("infra/x") == [{"number": 5, "title": "甲", "url": "u5", "isDraft": True}]


def test_codeup_edit_keeps_draft_prefix(codeup_env, tmp_path):
    body = tmp_path / "b.md"
    body.write_text("x", encoding="utf-8")
    host, fake = _codeup({"codeup-get-change-request": _mr(), "codeup-update-change-request": {}})
    host.edit("5", "新标题", str(body))
    args = fake.args_of("codeup-update-change-request")
    assert args[args.index("--title") + 1] == "[WIP] 新标题"


def test_codeup_view_normalizes(codeup_env):
    host, _ = _codeup({"codeup-get-change-request": _mr(), "codeup-list-change-request-patch-sets": _patch_sets()})
    view = host.view("5")
    assert view["title"] == "标题"
    assert view["isDraft"] is True
    assert view["headRefOid"] == HEAD
    assert (view["state"], view["baseRefName"]) == ("OPEN", "main")


def test_codeup_view_rejects_unknown_status(codeup_env):
    host, _ = _codeup({"codeup-get-change-request": _mr(status="WEIRD")})
    with pytest.raises(HostError, match="无法识别"):
        host.view("5")


def test_codeup_ready_strips_prefix_once(codeup_env):
    host, fake = _codeup({"codeup-get-change-request": _mr(), "codeup-update-change-request": {}})
    host.ready("5")
    args = fake.args_of("codeup-update-change-request")
    assert args[args.index("--title") + 1] == "标题"

    host, fake = _codeup({"codeup-get-change-request": _mr(title="标题")})
    host.ready("5")
    assert "codeup-update-change-request" not in fake.commands()


def _merge_replies(head_after_pass=HEAD, rules=None, merged_status="MERGED", reviewers=None):
    state = {"passed": False, "merged": False}

    def get(_cmd):
        if state["merged"]:
            return _mr(title="标题", status=merged_status, mergedRevision="d" * 40)
        return _mr(
            title="标题",
            author={"id": 1, "userId": "u-author"},
            reviewers=reviewers if reviewers is not None else [{"userId": "u-author"}],
            checkList={"requirementRuleItems": rules or [{"itemType": "R", "pass": True}]},
        )

    def patch_sets(_cmd):
        return _patch_sets(head_after_pass if state["passed"] else HEAD)

    def review(_cmd):
        state["passed"] = True
        return {}

    def merge(_cmd):
        state["merged"] = True
        return {}

    return {
        "codeup-get-change-request": get,
        "codeup-list-change-request-patch-sets": patch_sets,
        "codeup-review-change-request": review,
        "codeup-merge-change-request": merge,
        "codeup-update-change-request-related-person": {"result": True},
    }


def test_codeup_merge_passes_then_squashes(codeup_env):
    host, fake = _codeup(_merge_replies())
    result = host.merge("5", HEAD, False, "放行依据")
    assert result["state"] == "MERGED"
    assert result["mergeCommit"] == "d" * 40
    commands = fake.commands()
    assert commands.index("codeup-review-change-request") < commands.index("codeup-merge-change-request")
    review = fake.args_of("codeup-review-change-request")
    assert review[review.index("--review-opinion") + 1] == "PASS"
    merge = fake.args_of("codeup-merge-change-request")
    assert merge[merge.index("--merge-type") + 1] == "squash"
    assert merge[merge.index("--merge-message") + 1] == "标题"


def test_codeup_merge_refuses_admin_and_empty_note(codeup_env):
    host, fake = _codeup(_merge_replies())
    with pytest.raises(HostError, match="admin"):
        host.merge("5", HEAD, True, "x")
    with pytest.raises(HostError, match="note-file"):
        host.merge("5", HEAD, False, "  ")
    assert "codeup-review-change-request" not in fake.commands()


def test_codeup_merge_stops_when_head_moved_before_pass(codeup_env):
    host, fake = _codeup(_merge_replies())
    with pytest.raises(HostError, match="HEAD"):
        host.merge("5", OTHER, False, "x")
    assert "codeup-review-change-request" not in fake.commands()


def test_codeup_merge_stops_when_head_moved_after_pass(codeup_env):
    host, fake = _codeup(_merge_replies(head_after_pass=OTHER))
    with pytest.raises(HostError, match="HEAD"):
        host.merge("5", HEAD, False, "x")
    assert "codeup-merge-change-request" not in fake.commands()


def test_codeup_merge_stops_on_unmet_requirement(codeup_env):
    host, fake = _codeup(_merge_replies(rules=[{"itemType": "REVIEWER_APPROVED_CHECK", "pass": False}]))
    with pytest.raises(HostError, match="卡点"):
        host.merge("5", HEAD, False, "x")
    assert "codeup-merge-change-request" not in fake.commands()


def test_codeup_merge_refuses_draft(codeup_env):
    replies = _merge_replies()
    replies["codeup-get-change-request"] = _mr()
    host, _ = _codeup(replies)
    with pytest.raises(HostError, match="Draft"):
        host.merge("5", HEAD, False, "x")


def test_codeup_merge_requires_merged_readback(codeup_env):
    host, _ = _codeup(_merge_replies(merged_status="TO_BE_MERGED"))
    with pytest.raises(HostError, match="MERGED"):
        host.merge("5", HEAD, False, "x")


def test_codeup_checks_shape(codeup_env):
    ci = {
        "commit_statuses": [
            {"context": "lint", "state": "success"},
            {"context": "unit", "state": "failed", "target_url": "https://ci/1"},
            {"context": "build", "state": "running"},
        ],
        "check_runs": [{"name": "e2e", "status": "completed", "conclusion": "cancelled"}],
        "display_type": "X",
    }
    host, fake = _codeup(
        {
            "codeup-get-change-request": _mr(title="标题"),
            "codeup-list-change-request-patch-sets": _patch_sets(),
            "codeup-list-change-request-ci-check-list": ci,
        }
    )
    checks = host.checks("5")
    assert checks["head"] == HEAD
    assert (checks["mergeable"], checks["mergeStateStatus"], checks["isDraft"]) == ("MERGEABLE", "CLEAN", False)
    assert checks["checkCount"] == 4
    assert checks["passed"] == ["lint"]
    assert checks["pending"] == ["build"]
    assert [f["name"] for f in checks["failed"]] == ["unit"]
    assert checks["other"] == [{"name": "e2e", "conclusion": "CANCELLED"}]
    ci_args = fake.args_of("codeup-list-change-request-ci-check-list")
    assert ci_args[ci_args.index("--patch-set-biz-id") + 1] == "p2"


@pytest.mark.parametrize(
    ("mr", "expected"),
    [
        ({"conflictCheckStatus": "HAS_CONFLICT"}, "CONFLICTING"),
        ({"conflictCheckStatus": "CHECKING", "hasConflict": False}, "UNKNOWN"),
        ({"conflictCheckStatus": None, "hasConflict": True}, "CONFLICTING"),
    ],
)
def test_codeup_checks_mergeable_mapping(codeup_env, mr, expected):
    host, _ = _codeup(
        {
            "codeup-get-change-request": _mr(**mr),
            "codeup-list-change-request-patch-sets": _patch_sets(),
            "codeup-list-change-request-ci-check-list": {"commit_statuses": [], "check_runs": None},
        }
    )
    assert host.checks("5")["mergeable"] == expected


def test_codeup_checks_without_source_patch_set_fails(codeup_env):
    host, _ = _codeup({"codeup-get-change-request": _mr(), "codeup-list-change-request-patch-sets": []})
    with pytest.raises(HostError, match="MERGE_SOURCE"):
        host.checks("5")


def test_codeup_comments_render_only_open(codeup_env, tmp_path):
    rows = [
        {
            "comment_biz_id": "c1",
            "author": {"name": "评审"},
            "comment_type": "INLINE_COMMENT",
            "filePath": "a.py",
            "line_number": 3,
            "related_patchset": {"commitId": HEAD},
            "content": "这里会空指针",
            "child_comments_list": [{"author": {"name": "作者", "id": 9}, "content": "看一下", "comment_time": "1"}],
        },
        {
            "comment_biz_id": "c5",
            "content": "已裁决",
            "child_comments_list": [
                {"author": {"id": 1}, "content": "REJECT", "comment_time": "2"},
                {"author": {"id": 9}, "content": "早先的追问", "comment_time": "1"},
            ],
        },
        {
            "comment_biz_id": "c6",
            "content": "评审追问",
            "child_comments_list": [
                {"author": {"id": 1}, "content": "REJECT", "comment_time": "1"},
                {"author": {"id": 9}, "content": "不同意", "comment_time": "2"},
            ],
        },
        {"comment_biz_id": "c2", "resolved": True, "content": "已解决"},
        {"comment_biz_id": "c4", "content": "<!-- ai-review-placeholder-v1 -->\n智能评审进行中"},
        {"comment_biz_id": "c3", "is_deleted": True, "content": "删了"},
    ]
    host, _ = _codeup(
        {"codeup-list-change-request-patch-sets": _patch_sets(), "codeup-list-merge-request-comments": rows}
    )
    out = tmp_path / "c.md"
    result = host.comments("5", str(out))
    # 合并请求作者 id 1：c1 只有他人回复；c6 作者回复后评审又追问；c5 作者回复在最后，已裁决。
    assert (result["openCount"], result["openIds"], result["file"]) == (2, ["c1", "c6"], str(out))
    assert result["aiReviewPending"] is True
    text = out.read_text(encoding="utf-8")
    assert "a.py:3" in text and "这里会空指针" in text and "看一下" in text
    assert "已解决" not in text and "删了" not in text and "智能评审进行中" not in text and "已裁决" not in text


def test_codeup_comments_empty_has_no_file(codeup_env, tmp_path):
    host, _ = _codeup(
        {"codeup-list-change-request-patch-sets": _patch_sets(), "codeup-list-merge-request-comments": []}
    )
    result = host.comments("5", str(tmp_path / "c.md"))
    assert (result["file"], result["aiReviewPending"]) == (None, False)


def _parent(comment_type="INLINE_COMMENT"):
    row = {
        "comment_biz_id": "c1",
        "comment_type": comment_type,
        "related_patchset": {"patchSetBizId": "p1", "commitId": OTHER},
    }
    if comment_type == "INLINE_COMMENT":
        row |= {"filePath": "a.py", "line_number": 3, "from_patchset_biz_id": "base", "to_patchset_biz_id": "p1"}
    return row


def _reply_host(parent):
    return _codeup(
        {
            "codeup-list-merge-request-comments": [parent],
            "codeup-create-change-request-comment": {},
        }
    )


def test_codeup_reply_mirrors_inline_parent(codeup_env):
    host, fake = _reply_host(_parent())
    host.reply("5", "c1", "REJECT：……")
    create = fake.args_of("codeup-create-change-request-comment")
    expected = {
        "--comment-type": "INLINE_COMMENT",
        "--parent-comment-biz-id": "c1",
        # 回复挂在父评论的补丁版本上，而不是最新版本。
        "--patchset-biz-id": "p1",
        "--file-path": "a.py",
        "--line-number": "3",
        "--from-patchset-biz-id": "base",
        "--to-patchset-biz-id": "p1",
    }
    for flag, value in expected.items():
        assert create[create.index(flag) + 1] == value
    # 平台不允许作者解决他人的评论，回复之后不再尝试更新评论状态。
    assert "codeup-update-change-request-comment" not in fake.commands()


def test_codeup_reply_to_global_parent_has_no_inline_anchor(codeup_env):
    host, fake = _reply_host(_parent("GLOBAL_COMMENT"))
    host.reply("5", "c1", "HOUSEKEEPING")
    create = fake.args_of("codeup-create-change-request-comment")
    assert create[create.index("--comment-type") + 1] == "GLOBAL_COMMENT"
    assert "--file-path" not in create


def test_codeup_reply_unknown_parent_fails(codeup_env):
    host, fake = _reply_host(_parent())
    with pytest.raises(HostError, match="没有评论"):
        host.reply("5", "zz", "x")
    assert "codeup-create-change-request-comment" not in fake.commands()


def test_codeup_non_json_diagnostic_redacts_token(codeup_env, monkeypatch, capsys):
    def malformed(cmd, **kwargs):
        return subprocess.CompletedProcess(cmd, 0, "diagnostic token=tok-SECRET", "")

    host = pr_host_codeup.CodeupHost(pr_host.parse_remote(CODEUP_URL), runner=malformed)
    monkeypatch.setattr(pr_host, "detect", lambda: host)
    assert pr_host.main(["find", "--branch", "infra/x"]) == 2
    assert "tok-SECRET" not in capsys.readouterr().err


class CommentStore:
    """按调用回放评论存储：新建顶层评论进入列表，回复挂到父评论下。"""

    def __init__(self):
        self.rows = [
            {
                "comment_biz_id": "orig",
                "comment_type": "GLOBAL_COMMENT",
                "author": {"id": 2},
                "related_patchset": {"patchSetBizId": "p2"},
                "content": "请修复",
            }
        ]

    def __call__(self, cmd, **kwargs):
        def value(flag):
            return cmd[cmd.index(flag) + 1]

        command = cmd[2]
        if command == "codeup-get-repository":
            payload = {"id": 7607455}
        elif command == "codeup-get-change-request":
            payload = _mr(author={"id": 1})
        elif command == "codeup-list-change-request-patch-sets":
            payload = _patch_sets()
        elif command == "codeup-list-merge-request-comments":
            payload = self.rows
        elif command == "codeup-create-change-request-comment":
            row = {
                "comment_biz_id": f"n{len(self.rows)}",
                "author": {"id": 1},
                "comment_time": "9",
                "resolved": value("--resolved") == "true",
                "content": value("--content"),
            }
            if "--parent-comment-biz-id" in cmd:
                parent = next(r for r in self.rows if r["comment_biz_id"] == value("--parent-comment-biz-id"))
                parent.setdefault("child_comments_list", []).append(row)
            else:
                self.rows.append(row)
            payload = row
        else:
            raise AssertionError(command)
        return subprocess.CompletedProcess(cmd, 0, json.dumps(payload), "")


def test_codeup_reply_then_ledger_keeps_comments_converged(codeup_env, tmp_path):
    host = pr_host_codeup.CodeupHost(pr_host.parse_remote(CODEUP_URL), runner=CommentStore())
    assert host.comments("5", None)["openCount"] == 1
    host.reply("5", "orig", "REJECT：一手反证")
    ledger = tmp_path / "triage.md"
    ledger.write_text("# 台账\n", encoding="utf-8")
    host.comment("5", str(ledger))
    assert host.comments("5", None)["openCount"] == 0


def test_codeup_merge_adds_author_as_reviewer_keeping_others(codeup_env):
    host, fake = _codeup(_merge_replies(reviewers=[{"userId": "u-other"}]))
    host.merge("5", HEAD, False, "放行依据")
    commands = fake.commands()
    assert commands.index("codeup-update-change-request-related-person") < commands.index(
        "codeup-review-change-request"
    )
    args = fake.args_of("codeup-update-change-request-related-person")
    assert args[args.index("--type") + 1] == "REVIEWER"
    assert args[args.index("--user-ids") + 1 :] == ["u-other", "u-author"]


def test_codeup_merge_skips_reviewer_update_when_author_already_reviews(codeup_env):
    host, fake = _codeup(_merge_replies())
    host.merge("5", HEAD, False, "放行依据")
    assert "codeup-update-change-request-related-person" not in fake.commands()


def test_profile_reads_main_worktree_and_defaults(tmp_path):
    repo = tmp_path / "repo"
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    assert pr_host.repository_policy(repo) == {"gate_command": None, "require_exact_head_gate": False}
    override = repo / ".bus/repo.toml"
    override.parent.mkdir(parents=True)
    override.write_text('gate_command = "just ci"\nrequire_exact_head_gate = true\n')
    assert pr_host.repository_policy(repo) == {"gate_command": "just ci", "require_exact_head_gate": True}
    assert not (repo / "temp").exists()


def test_profile_scans_linked_worktree_without_writing_main_cache(tmp_path):
    repo, linked = tmp_path / "repo", tmp_path / "linked"
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    override = repo / ".bus/repo.toml"
    override.parent.mkdir(parents=True)
    override.write_text('gate_command = "custom gate"\nrequire_exact_head_gate = true\n')
    subprocess.run(["git", "-C", str(repo), "add", ".bus/repo.toml"], check=True)
    subprocess.run(["git", "-C", str(repo), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "-qm", "fixture"], check=True)
    subprocess.run(["git", "-C", str(repo), "worktree", "add", "-qb", "feat/linked", str(linked)], check=True)
    assert pr_host.repository_policy(linked) == {"gate_command": "custom gate", "require_exact_head_gate": True}
    assert not (repo / "temp").exists() and not (linked / "temp").exists()


@pytest.mark.parametrize("policy", [{"require_exact_head_gate": "false"}, {"gate_command": []}])
def test_profile_rejects_wrong_policy_types(tmp_path, policy):
    repo = tmp_path / "repo"
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    override = repo / ".bus/repo.toml"
    override.parent.mkdir(parents=True)
    override.write_text("".join(f"{key} = {json.dumps(value)}\n" for key, value in policy.items()))
    with pytest.raises(HostError):
        pr_host.repository_policy(repo)


@pytest.fixture(autouse=True)
def isolated_home(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")


def test_recorded_github_view_replay(gh_calls):
    calls, replies = gh_calls
    fixture = Path(__file__).parent / "fixtures/github-view.json"
    replies[("pr", "view")] = fixture.read_text()
    result = _github().view("42")
    assert result["state"] == "MERGED" and result["mergeCommit"] == OTHER
    assert result["headRefOid"] == HEAD


def test_recorded_codeup_view_replay(codeup_env):
    fixtures = Path(__file__).parent / "fixtures"
    host, fake = _codeup({
        "codeup-get-change-request": json.loads((fixtures / "codeup-view.json").read_text()),
        "codeup-list-change-request-patch-sets": json.loads((fixtures / "codeup-patches.json").read_text()),
    })
    result = host.view("5")
    assert result["state"] == "MERGED" and result["mergeCommit"] == OTHER
    assert result["headRefOid"] == HEAD
    recorded = json.loads((fixtures / "codeup-view.json").read_text())
    assert result["mergedAt"] == recorded["updateTime"]
    assert result["mergedAt"] == "2026-10-26T11:17:00+08:00"


def test_recorded_codeup_view_selects_latest_source_patch_set(codeup_env):
    fixtures = Path(__file__).parent / "fixtures"
    patches = json.loads((fixtures / "codeup-patches.json").read_text())
    stale = [p["commitId"] for p in patches if (p["relatedMergeItemType"], p["versionNo"]) != ("MERGE_SOURCE", 2)]
    latest = next(p["commitId"] for p in patches if (p["relatedMergeItemType"], p["versionNo"]) == ("MERGE_SOURCE", 2))
    assert latest == HEAD
    assert latest not in stale, "fixture cannot distinguish older or target patch sets"


def test_profile_honors_checked_in_override_without_scan_cache(tmp_path):
    repo = tmp_path / "repo"
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    override = repo / ".bus/repo.toml"
    override.parent.mkdir(parents=True)
    override.write_text('gate_command = "just ci"\nrequire_exact_head_gate = true\n')
    assert pr_host.repository_policy(repo) == {"gate_command": "just ci", "require_exact_head_gate": True}
    assert not (repo / "temp").exists()


def test_profile_survives_unrelated_manifest_hazard(tmp_path):
    repo = tmp_path / "repo"
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    override = repo / ".bus/repo.toml"
    override.parent.mkdir(parents=True)
    override.write_text('gate_command = "just ci"\nrequire_exact_head_gate = true\n')
    (repo / "package.json").write_text("{}")
    (repo / "package-lock.json").write_text("{}")
    (repo / "yarn.lock").write_text("")
    assert pr_host.repository_policy(repo) == {"gate_command": "just ci", "require_exact_head_gate": True}


def test_profile_ignores_stale_cache_without_rewriting_it(tmp_path):
    repo = tmp_path / "repo"
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    override = repo / ".bus/repo.toml"
    override.parent.mkdir(parents=True)
    override.write_text('gate_command = "old gate"\nrequire_exact_head_gate = false\n')
    scanner = Path(pr_host.__file__).resolve().parents[2] / "worktree-new/scripts/repo_profile.py"
    subprocess.run([sys.executable, str(scanner), "scan", "--repo", str(repo)], check=True, capture_output=True)
    cache = repo / "temp/.bus/repo-profile.json"
    before = cache.read_bytes()
    override.write_text('gate_command = "just ci"\nrequire_exact_head_gate = true\n')
    assert pr_host.repository_policy(repo) == {"gate_command": "just ci", "require_exact_head_gate": True}
    assert cache.read_bytes() == before


@pytest.mark.parametrize("layout", [".claude/skills", ".agents/skills"])
def test_profile_cli_uses_installed_schema_owner(tmp_path, layout):
    import shutil

    support = Path(pr_host.__file__).resolve().parent
    skills = tmp_path / layout
    installed = skills / "_bus/cli_extensions"
    installed.mkdir(parents=True)
    for filename in ("pr_host.py", "pr_host_common.py", "pr_host_codeup.py"):
        shutil.copyfile(support / filename, installed / filename)
    source = support.parents[1] / "worktree-new/scripts"
    shutil.copytree(source, skills / "worktree-new/scripts", ignore=shutil.ignore_patterns("__pycache__", "tests"))
    repo = tmp_path / "foreign repo"
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    override = repo / ".bus/repo.toml"
    override.parent.mkdir(parents=True)
    override.write_text('gate_command = "just ci"\nrequire_exact_head_gate = true\n')
    done = subprocess.run([sys.executable, str(installed / "pr_host.py"), "profile"], cwd=repo,
                          text=True, capture_output=True)
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout) == {"policy": {"gate_command": "just ci", "require_exact_head_gate": True}}
    assert not (repo / "temp").exists()
