"""Fixed whole-file targets shared by the reviewer and author-side sanitizer."""

from __future__ import annotations

import difflib
import hashlib
import json
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


class ReviewScopeError(ValueError):
    """A chunk cannot be identified or snapshotted safely."""


def is_test_file(path: str) -> bool:
    # Bus keeps test bodies in dedicated *_test files, including Rust unit modules.
    file = Path(path)
    return file.stem.endswith("_test") and file.suffix in (".rs", ".py", ".ts", ".tsx", ".js", ".jsx")


def _paths(values: object, repo: Path, field: str) -> tuple[str, ...]:
    if not isinstance(values, list) or not all(isinstance(p, str) for p in values):
        raise ReviewScopeError(f"scope {field} 必须是文件路径数组")
    lexical_root = repo.absolute()
    repo = repo.resolve()
    result: set[str] = set()
    for value in values:
        if not value or any(c in value for c in ("\r", "\n", "\0", ",", "\\")):
            raise ReviewScopeError(f"scope 路径为空或含不支持的字符: {value!r}")
        path = PurePosixPath(value)
        if ".." in path.parts:
            raise ReviewScopeError(f"scope 路径不得含 ..: {value}")
        if path.is_absolute():
            try:
                try:
                    relative = Path(value).relative_to(lexical_root)
                except ValueError:
                    relative = Path(value).relative_to(repo)
                path = PurePosixPath(relative.as_posix())
            except ValueError as error:
                raise ReviewScopeError(f"scope 路径不在仓库内: {value}") from error
        if not path.parts or path.parts[0] in (".git", "temp", "plans") or path.parts[:2] == ("docs", "plans"):
            raise ReviewScopeError(f"scope 不接受目录、临时产物或计划: {value}")
        normalized = path.as_posix()
        # Reviewers may write only these paths; a working-tree symlink must not redirect a probe.
        try:
            (repo / normalized).resolve().relative_to(repo.resolve())
        except ValueError as error:
            raise ReviewScopeError(f"scope 路径通过 symlink 越出仓库: {value}") from error
        if any(parent.is_symlink() for parent in (repo / normalized, *(repo / normalized).parents)):
            raise ReviewScopeError(f"scope 不接受 symlink 路径: {value}")
        result.add(normalized)
    return tuple(sorted(result))


@dataclass(frozen=True)
class ReviewScope:
    files: tuple[str, ...]
    test_files: tuple[str, ...]

    @property
    def manifest(self) -> dict[str, list[str]]:
        return {"files": list(self.files), "test_files": list(self.test_files)}

    @property
    def scope_hash(self) -> str:
        canonical = json.dumps(self.manifest, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        return hashlib.sha256(canonical.encode("utf-8")).hexdigest()

    def write(self, path: Path) -> None:
        path.write_text(json.dumps(self.manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")


def load_review_scope(path: Path, repo: Path) -> ReviewScope:
    text = path.read_text(encoding="utf-8")
    if text.lstrip().startswith(("{", "[")):
        try:
            data = json.loads(text)
        except ValueError as error:
            raise ReviewScopeError(f"scope JSON 无效: {path}") from error
        if isinstance(data, list):
            data = {"files": data}
        if not isinstance(data, dict) or set(data) - {"files", "test_files"}:
            raise ReviewScopeError("scope manifest 只接受 files / test_files")
    else:
        data = {"files": [line.strip() for line in text.splitlines() if line.strip() and not line.lstrip().startswith("#")]}
    files = _paths(data.get("files"), repo, "files")
    if not files:
        raise ReviewScopeError("scope files 不能为空")
    tests = _paths(data.get("test_files", [p for p in files if is_test_file(p)]), repo, "test_files")
    if any(not is_test_file(p) for p in tests):
        raise ReviewScopeError("scope test_files 必须是专用 *_test 文件；不得授权写生产文件")
    # Explicit probe paths are part of the chunk too, even before the author adopts them.
    return ReviewScope(tuple(sorted(set(files) | set(tests))), tests)


def snapshot_scope(scope: ReviewScope, repo: Path, head: str) -> dict:
    """Read only pinned Git blobs, never dirty source or uncommitted reviewer probes."""
    files: dict[str, dict | None] = {}
    for path in scope.files:
        result = subprocess.run(
            ["git", "ls-tree", "-z", head, "--", f":(literal){path}"],
            cwd=repo, capture_output=True, check=True,
        )
        if not result.stdout:
            files[path] = None
            continue
        metadata, name = result.stdout.rstrip(b"\0").split(b"\t", 1)
        mode, kind, oid = metadata.decode("ascii").split()
        if name.decode("utf-8") != path or kind != "blob" or mode not in ("100644", "100755"):
            raise ReviewScopeError(f"scope 只接受普通文件: {path}")
        blob = subprocess.run(["git", "cat-file", "blob", oid], cwd=repo, capture_output=True, check=True).stdout
        try:
            text = blob.decode("utf-8")
        except UnicodeError as error:
            raise ReviewScopeError(f"scope 文件不是 UTF-8: {path}") from error
        if "\0" in text:
            raise ReviewScopeError(f"scope 不接受二进制文件: {path}")
        files[path] = {"mode": mode, "text": text}
    return {"head": head, "files": files}


def load_scope_snapshot(path: Path, scope: ReviewScope) -> dict:
    try:
        snapshot = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(snapshot, dict) or re.fullmatch(r"[0-9a-f]{40,64}", snapshot.get("head", "")) is None:
            raise ValueError("invalid snapshot identity")
        files = snapshot["files"]
        if not isinstance(files, dict) or set(files) != set(scope.files):
            raise ValueError("file set mismatch")
        for entry in files.values():
            if entry is not None and (
                not isinstance(entry, dict) or entry["mode"] not in ("100644", "100755") or not isinstance(entry["text"], str)
            ):
                raise ValueError("invalid file entry")
        return snapshot
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise ReviewScopeError(f"缺少或损坏的前轮 scope snapshot: {path}") from error


def scope_patch(previous: dict | None, current: dict) -> str:
    """Round 1 emits all lines; later rounds diff file contents, not a diff's line numbers."""
    old_files = previous["files"] if previous else {}
    parts: list[str] = []
    for path, entry in current["files"].items():
        old = old_files.get(path)
        if previous is not None and old == entry:
            continue
        parts.append(f"diff --git a/{path} b/{path}\n")
        if old is None and entry is not None:
            parts.append(f"new file mode {entry['mode']}\n")
        elif entry is None and old is not None:
            parts.append(f"deleted file mode {old['mode']}\n")
        elif old and entry and old["mode"] != entry["mode"]:
            parts.extend([f"old mode {old['mode']}\n", f"new mode {entry['mode']}\n"])
        for line in difflib.unified_diff(
            old["text"].splitlines(keepends=True) if old else [],
            entry["text"].splitlines(keepends=True) if entry else [],
            fromfile=f"a/{path}" if old else "/dev/null",
            tofile=f"b/{path}" if entry else "/dev/null",
        ):
            parts.append(line if line.endswith("\n") else line + "\n\\ No newline at end of file\n")
    return "".join(parts)
