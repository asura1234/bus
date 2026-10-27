#!/usr/bin/env python3
"""`pr` / `merge-pr` 访问代码托管平台的唯一入口：按 `origin` 判定平台，再把 PR 操作翻译成
对应 CLI。

- `github.com` → `gh`。命令与 skill 迁移前逐字相同，GitHub 行为不因 Codeup 支持而改变。
- `codeup.aliyun.com` → `aliyun devops codeup-*`，见 `pr_host_codeup.py`。
- 其他主机 → 直接失败。猜一个平台去调用，等于把 PR 发到不知道的地方。

所有子命令输出一个 JSON 对象（`find` 输出数组）。下列通用子命令两端字段同形，调用方不必区分平台；
`checks` / `comments` / `reply` 只有 Codeup 实现，在 GitHub 上直接失败，调用方须先按 `host` 分支：

  host                                          当前平台
  find    --branch B                            该分支上 open 的 PR
  create  --branch B --base main --title T --body-file F   创建 Draft PR
  edit    --pr N --title T --body-file F        改标题与正文，不改 Draft 状态
  view    --pr N                                标题、正文、HEAD、状态、base、Draft、合并提交
  ready   --pr N                                Draft → Ready
  comment --pr N --body-file F                  发一条 PR 级评论
  request-review --pr N                         请求机器评审（GitHub Copilot / Codeup AI 评审）
  merge   --pr N --head SHA [--admin] [--note-file F]       squash 合并并读回
  checks / comments / reply                     仅 Codeup；GitHub 的对应信号归 merge-pr 与
                                                address-review-comments 的既有脚本
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parent))

from pr_host_common import HostError, Remote  # noqa: E402


GITHUB_HOST = "github.com"
CODEUP_HOST = "codeup.aliyun.com"

REMOTE_RE = re.compile(
    r"^(?:(?:https?|ssh)://(?:[^@/]+@)?(?P<h1>[^/:]+)(?::\d+)?/|[^@\s]+@(?P<h2>[^:/]+):)(?P<path>.+?)(?:\.git)?/?$"
)


def parse_remote(url: str) -> Remote:
    """只认 https / ssh / scp 三种写法；用户名与凭据段不进入结果。"""

    match = REMOTE_RE.match(url.strip())
    if not match:
        raise HostError(f"无法解析 origin 地址：{_strip_credentials(url)}")
    host = (match.group("h1") or match.group("h2")).lower()
    return Remote(host=host, path=match.group("path"), url=_strip_credentials(url.strip()))


def _strip_credentials(url: str) -> str:
    return re.sub(r"(?<=://)[^@/]+@", "", url)


def origin_remote(repo: Path | None = None) -> Remote:
    cmd = ["git", "remote", "get-url", "origin"]
    if repo is not None:
        cmd[1:1] = ["-C", str(repo)]
    done = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise HostError(f"读取 origin 失败：{done.stderr.strip()}")
    return parse_remote(done.stdout)


def _gh(*args: str) -> str:
    done = subprocess.run(["gh", *args], capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise HostError(f"gh {' '.join(args)} 失败：{done.stderr.strip()}")
    return done.stdout


class GitHubHost:
    name = "github"

    def __init__(self, remote: Remote):
        self.remote = remote

    def find(self, branch: str) -> list[dict]:
        return json.loads(_gh("pr", "list", "--head", branch, "--state", "open", "--json", "number,title,url,isDraft"))

    def create(self, branch: str, base: str, title: str, body_file: str) -> dict:
        out = _gh(
            "pr", "create", "--draft", "--head", branch, "--base", base, "--title", title, "--body-file", body_file
        )
        url = out.strip().splitlines()[-1].strip() if out.strip() else ""
        number = url.rstrip("/").rsplit("/", 1)[-1]
        if not number.isdigit():
            raise HostError(f"gh pr create 没有返回 PR 地址：{out.strip()!r}")
        return {"number": int(number), "url": url, "isDraft": True}

    def edit(self, pr: str, title: str, body_file: str) -> dict:
        _gh("pr", "edit", pr, "--title", title, "--body-file", body_file)
        return {"number": int(pr)}

    def view(self, pr: str) -> dict:
        data = json.loads(
            _gh(
                "pr",
                "view",
                pr,
                "--json",
                "number,url,title,body,headRefName,headRefOid,state,baseRefName,isDraft,mergedAt,mergeCommit",
            )
        )
        merge_commit = data.get("mergeCommit") or {}
        data["mergeCommit"] = merge_commit.get("oid") if isinstance(merge_commit, dict) else merge_commit
        return data

    def ready(self, pr: str) -> dict:
        _gh("pr", "ready", pr)
        return {"number": int(pr), "isDraft": False}

    def comment(self, pr: str, body_file: str) -> dict:
        _gh("pr", "comment", pr, "--body-file", body_file)
        return {"number": int(pr)}

    def request_review(self, pr: str) -> dict:
        # `@copilot` 是 gh 的新写法；老版本 gh 只认 bot 的登录名。
        try:
            _gh("pr", "edit", pr, "--add-reviewer", "@copilot")
        except HostError:
            _gh("pr", "edit", pr, "--add-reviewer", "copilot-pull-request-reviewer")
        return {"number": int(pr), "reviewer": "copilot"}

    def merge(self, pr: str, head: str, admin: bool, note: str | None) -> dict:
        del note  # GitHub 的放行依据写在 merge-pr 的输出里，不需要平台侧登记。
        args = ["pr", "merge", pr, "--squash"]
        if admin:
            args.append("--admin")
        _gh(*args, "--delete-branch=false", "--match-head-commit", head)
        return self.view(pr)


def detect(repo: Path | None = None):
    remote = origin_remote(repo)
    if remote.host == GITHUB_HOST:
        return GitHubHost(remote)
    if remote.host == CODEUP_HOST:
        # Codeup 实现只在需要时加载，GitHub 路径不受它的凭据检查影响。
        from pr_host_codeup import CodeupHost  # pylint: disable=import-outside-toplevel

        return CodeupHost(remote)
    raise HostError(
        f"不支持的代码托管平台：{remote.host}（origin = {remote.url}）；只支持 {GITHUB_HOST} 与 {CODEUP_HOST}"
    )


def _codeup_only(host, command: str):
    if host.name != "codeup":
        raise HostError(f"`{command}` 只用于 Codeup；GitHub 走 merge-pr / address-review-comments 的既有脚本")
    return host


def repository_policy(repo: Path | None = None) -> dict:
    """Read policy from the main worktree; missing profiles use upstream defaults."""
    done = subprocess.run(["git", "worktree", "list", "--porcelain", "-z"], cwd=repo,
                          capture_output=True, check=False)
    if done.returncode or not done.stdout.startswith(b"worktree "):
        raise HostError("Cannot resolve the target repository's main worktree")
    main = Path(os.fsdecode(done.stdout.split(b"\0", 1)[0].removeprefix(b"worktree ")))
    path = main / "temp/.bus/repo-profile.json"
    policy = {}
    if path.is_file():
        try:
            policy = json.loads(path.read_text(encoding="utf-8"))["policy"]
        except (OSError, ValueError, KeyError, TypeError) as error:
            raise HostError("Malformed repository profile") from error
    if not isinstance(policy, dict):
        raise HostError("Malformed repository policy")
    command = policy.get("gate_command")
    required = policy.get("require_exact_head_gate", False)
    if (command is not None and (not isinstance(command, str) or not command.strip())) or type(required) is not bool:
        raise HostError("Invalid gate_command or require_exact_head_gate policy")
    return {"gate_command": command, "require_exact_head_gate": required}


def run(args: argparse.Namespace, host) -> object:
    command = args.command
    if command == "host":
        return {"host": host.name, "remote": host.remote.url, "path": host.remote.path}
    if command == "find":
        return host.find(args.branch)
    if command == "create":
        return host.create(args.branch, args.base, args.title, args.body_file)
    if command == "edit":
        return host.edit(args.pr, args.title, args.body_file)
    if command == "view":
        return host.view(args.pr)
    if command == "ready":
        return host.ready(args.pr)
    if command == "comment":
        return host.comment(args.pr, args.body_file)
    if command == "request-review":
        return host.request_review(args.pr)
    if command == "merge":
        note = Path(args.note_file).read_text(encoding="utf-8") if args.note_file else None
        return host.merge(args.pr, args.head, args.admin, note)
    if command == "checks":
        return _codeup_only(host, command).checks(args.pr)
    if command == "comments":
        return _codeup_only(host, command).comments(args.pr, args.output)
    if command == "reply":
        body = Path(args.body_file).read_text(encoding="utf-8")
        return _codeup_only(host, command).reply(args.pr, args.comment_id, body)
    raise HostError(f"未知子命令：{command}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("host")
    sub.add_parser("profile")
    sub.add_parser("find").add_argument("--branch", required=True)

    create = sub.add_parser("create")
    create.add_argument("--branch", required=True)
    create.add_argument("--base", default="main")
    create.add_argument("--title", required=True)
    create.add_argument("--body-file", required=True)

    edit = sub.add_parser("edit")
    edit.add_argument("--pr", required=True)
    edit.add_argument("--title", required=True)
    edit.add_argument("--body-file", required=True)

    for name in ("view", "ready", "request-review", "checks"):
        sub.add_parser(name).add_argument("--pr", required=True)

    comment = sub.add_parser("comment")
    comment.add_argument("--pr", required=True)
    comment.add_argument("--body-file", required=True)

    merge = sub.add_parser("merge")
    merge.add_argument("--pr", required=True)
    merge.add_argument("--head", required=True, help="完整 40 位 sha")
    merge.add_argument("--admin", action="store_true", help="仅 GitHub；Codeup 的放行依据走 --note-file")
    merge.add_argument("--note-file", default=None, help="Codeup 登记“通过”时的评审备注（必填）")

    comments = sub.add_parser("comments")
    comments.add_argument("--pr", required=True)
    comments.add_argument("--output", default=None, help="把未解决评论渲染成 free-form 评审文件")

    reply = sub.add_parser("reply")
    reply.add_argument("--pr", required=True)
    reply.add_argument("--comment-id", required=True)
    reply.add_argument("--body-file", required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if getattr(args, "head", None) is not None and not re.fullmatch(r"[0-9a-f]{40}", args.head):
        print("error: --head 必须是完整 40 位小写 sha", file=sys.stderr)
        return 2
    try:
        payload = {"policy": repository_policy()} if args.command == "profile" else run(args, detect())
    except HostError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(payload, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
