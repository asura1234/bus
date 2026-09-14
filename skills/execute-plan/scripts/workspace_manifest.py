#!/usr/bin/env python3
"""execute-plan 自有的 Git worktree snapshot 与原子 JSON 工具。"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping


CONTROL_STATE_RE = re.compile(r"^temp/execute-plan/")


class WorkspaceManifestError(ValueError):
    """Git workspace 或 snapshot 不满足机械契约。"""


@dataclass(frozen=True)
class ManifestEntry:
    path: str
    status: str
    kind: str
    content_hash: str | None
    previous_path: str | None = None


def git(repo: Path, *args: str, env: Mapping[str, str] | None = None) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        env=dict(env) if env is not None else None,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "git command failed"
        raise WorkspaceManifestError(detail)
    return result.stdout.strip()


def canonical_repo_root(path: Path) -> Path:
    candidate = path.expanduser().resolve()
    if not candidate.is_dir():
        raise WorkspaceManifestError(f"repo 不存在或不是目录：{candidate}")
    root = Path(git(candidate, "rev-parse", "--show-toplevel")).resolve()
    if root != candidate:
        raise WorkspaceManifestError(f"repo 必须是 canonical git root：{candidate}")
    return root


def head(repo: Path) -> str:
    value = git(repo, "rev-parse", "HEAD")
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise WorkspaceManifestError("HEAD 不是 40 位 commit hash")
    return value


def _hash_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def _hash_directory(path: Path) -> str:
    digest = hashlib.sha256()
    for child in sorted(path.rglob("*"), key=lambda item: item.as_posix()):
        relative = child.relative_to(path)
        if ".git" in relative.parts:
            continue
        digest.update(relative.as_posix().encode())
        if child.is_symlink():
            digest.update(b"L")
            digest.update(os.fsencode(os.readlink(child)))
        elif child.is_file():
            digest.update(b"F")
            digest.update(child.read_bytes())
        elif child.is_dir():
            digest.update(b"D")
    return digest.hexdigest()


def _hash_nested_repository(path: Path) -> str:
    """按嵌套仓库的 git 可见状态取 hash，而不是递归目录内容。

    submodule 目录里混着 node_modules、构建产物与测试缓存，它们都被 .gitignore 排除、
    也不进任何提交。用 _hash_directory 递归整个目录会把这些噪音算进去：任务只要在
    submodule 内跑一次自己的验收测试（vitest 会重写 node_modules/.vite/**/results.json），
    该条目的 hash 就变，driver 于是算出一个恒不为空、且内容只有目录路径本身的 generation
    delta——而 touched_content_hash 只接受 file/symlink/deleted，报告既列不了它也删不掉它，
    任何状态的 report 都无法 ingest。改按 HEAD + git status 记录取 hash：ignored 产物天然
    不在其中，tracked 改动与未跟踪新文件仍然逐字参与。
    """

    digest = hashlib.sha256()
    digest.update(b"HEAD\0")
    digest.update(git(path, "rev-parse", "HEAD").encode())
    digest.update(b"\0")
    result = subprocess.run(
        [
            "git",
            "-C",
            str(path),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
        capture_output=True,
        check=False,
    )
    for digest_bytes, relative in sorted(_porcelain_v1_records(result.stdout)):
        digest.update(digest_bytes)
        digest.update(b"\0")
        child = path / os.fsdecode(relative)
        if child.is_symlink():
            digest.update(b"L")
            digest.update(os.fsencode(os.readlink(child)))
        elif child.is_file():
            digest.update(b"F")
            digest.update(child.read_bytes())
        else:
            digest.update(b"D")
        digest.update(b"\0")
    return digest.hexdigest()


def _porcelain_v1_records(stdout: bytes) -> list[tuple[bytes, bytes]]:
    """把 `git status --porcelain=v1 -z` 的输出切成 (摘要字节, 路径) 对。

    `-z` 下 rename/copy 不是一条 record：它是 `R<X> <new>\0<orig>\0` 两个 NUL 结尾的字段。
    直接按 `\0` split 会把 `<orig>` 也当成一条 record，而它没有那 3 字节的 `XY ` 前缀，于是
    `record[3:]` 会从路径里啃掉三个字符——实测 `git mv a.txt b.txt` 得到 `R  b.txt\0a.txt\0`，
    `a.txt` 被解析成 `xt`。

    摘要字节保留**两个**字段（原字段间的 NUL 一并保留），否则「从哪里改名过来」这一位状态会从
    hash 里消失；路径只取 `<new>`，因为 `<orig>` 已经不在工作树上，stat 它没有意义。
    """

    fields = [item for item in stdout.split(b"\0") if item]
    records: list[tuple[bytes, bytes]] = []
    index = 0
    while index < len(fields):
        record = fields[index]
        index += 1
        digest_bytes = record
        # 索引态 rename/copy 才有第二个字段；工作树侧的 R/C 由 `XY` 的第二位表达，不额外带路径。
        if record[:1] in (b"R", b"C") and index < len(fields):
            digest_bytes = record + b"\0" + fields[index]
            index += 1
        records.append((digest_bytes, record[3:]))
    return records


def _path_facts(path: Path) -> tuple[str, str | None]:
    if path.is_symlink():
        return "symlink", _hash_bytes(os.fsencode(os.readlink(path)))
    if path.is_file():
        return "file", _hash_bytes(path.read_bytes())
    if path.is_dir():
        if (path / ".git").exists():
            return "directory", _hash_nested_repository(path)
        return "directory", _hash_directory(path)
    return "missing", None


def _porcelain_records(
    repo: Path,
    *extra: str,
) -> list[tuple[str, str, str | None]]:
    result = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            *extra,
        ],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise WorkspaceManifestError(
            result.stderr.decode(errors="replace").strip()
        )
    fields = result.stdout.split(b"\0")
    records: list[tuple[str, str, str | None]] = []
    index = 0
    while index < len(fields):
        raw = fields[index]
        index += 1
        if not raw:
            continue
        if len(raw) < 4 or raw[2:3] != b" ":
            raise WorkspaceManifestError("无法解析 git status porcelain 输出")
        status = raw[:2].decode(errors="replace")
        path = raw[3:].decode(errors="surrogateescape")
        previous: str | None = None
        if "R" in status or "C" in status:
            if index >= len(fields) or not fields[index]:
                raise WorkspaceManifestError("git rename/copy status 缺少原路径")
            previous = fields[index].decode(errors="surrogateescape")
            index += 1
        records.append((path, status, previous))
    return records


def _status_records(repo: Path) -> list[tuple[str, str, str | None]]:
    """父仓状态；嵌套仓库展开成逐文件条目。

    父仓的 `git status` 只把整个 submodule 报成**一条目录路径**，于是 manifest 里它是一个
    `kind="directory"` 条目。可 `touched_content_hash` 只接受 file / symlink / deleted，
    task report 因此既列不了这条目录、也不能不列它——owner 覆盖它就得到一个无法表达的
    delta，不覆盖它就是 owner 越界。两条路都堵死，且 delta 校验排在 status 分支之前，连
    OWNER_GAP 都发不出去。展开成逐文件条目后，submodule 内的改动就是普通文件条目：
    ownership、fingerprint 与 scoped lint 都按原样工作。

    仅当嵌套仓库工作树干净（只有 HEAD 移动）时保留原目录条目，否则那次 gitlink 移动会
    在 manifest 里彻底消失。
    """

    records: list[tuple[str, str, str | None]] = []
    for path, status, previous in _porcelain_records(repo, "--ignore-submodules=none"):
        target = repo / path
        if target.is_dir() and (target / ".git").exists():
            prefix = Path(path)
            nested = _porcelain_records(target)
            if nested:
                for inner, inner_status, inner_previous in nested:
                    records.append(
                        (
                            (prefix / inner).as_posix(),
                            inner_status,
                            (prefix / inner_previous).as_posix()
                            if inner_previous
                            else None,
                        )
                    )
                continue
        records.append((path, status, previous))
    return [
        record
        for record in records
        if not CONTROL_STATE_RE.match(Path(record[0]).as_posix())
    ]


def capture_entries(repo_raw: Path) -> tuple[ManifestEntry, ...]:
    repo = canonical_repo_root(repo_raw)
    entries = []
    for path_text, status, previous in _status_records(repo):
        kind, content_hash = _path_facts(repo / path_text)
        entries.append(
            ManifestEntry(
                path=Path(path_text).as_posix(),
                status=status,
                kind=kind,
                content_hash=content_hash,
                previous_path=Path(previous).as_posix() if previous else None,
            )
        )
    return tuple(
        sorted(entries, key=lambda item: (item.path, item.status, item.previous_path or ""))
    )


def entry_to_dict(entry: ManifestEntry) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "path": entry.path,
        "status": entry.status,
        "kind": entry.kind,
        "content_hash": entry.content_hash,
    }
    if entry.previous_path is not None:
        payload["previous_path"] = entry.previous_path
    return payload


def stable_json(payload: Any) -> str:
    return json.dumps(
        payload,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ) + "\n"


def atomic_create_json(path: Path, payload: dict[str, Any]) -> bool:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            stream.write(json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True))
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.link(temporary, path)
        except FileExistsError:
            return False
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
        return True
    finally:
        temporary.unlink(missing_ok=True)


def worktree_tree_excluding(
    repo_raw: Path,
    excluded_paths: tuple[str, ...] = (),
) -> str:
    repo = canonical_repo_root(repo_raw)
    with tempfile.TemporaryDirectory(prefix="execute-plan-tree-") as directory:
        env = os.environ.copy()
        env["GIT_INDEX_FILE"] = str(Path(directory) / "index")
        git(repo, "read-tree", "HEAD", env=env)
        git(repo, "add", "-A", "--", ".", env=env)
        git(repo, "reset", "-q", "HEAD", "--", ":(glob)temp/execute-plan/**", env=env)
        if excluded_paths:
            git(repo, "reset", "-q", "HEAD", "--", *excluded_paths, env=env)
        tree = git(repo, "write-tree", env=env)
    if not re.fullmatch(r"[0-9a-f]{40}", tree):
        raise WorkspaceManifestError("worktree tree 不是 40 位 tree hash")
    return tree
