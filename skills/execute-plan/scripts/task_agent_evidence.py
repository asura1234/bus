"""Task agent gate 与内容证据的机械校验。"""

from __future__ import annotations

import hashlib
import os
import re
import shlex
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


NONE = "无。"
GATE_KINDS = frozenset({"test", "coverage", "typecheck", "build", "check"})
_GATE_HEADER_RE = re.compile(r"^### Gate ([1-9][0-9]*)$")
_COMMAND_RE = re.compile(r"^- COMMAND: `(.+)`$")
_OWNER_RE = re.compile(r"^- OWNER: task-agent$")
_LEVEL_RE = re.compile(r"^- LEVEL: TASK_LOCAL$")
_WORKDIR_RE = re.compile(r"^- WORKDIR: `(.+)`$")
_KIND_RE = re.compile(r"^- KIND: ([a-z]+)$")
_EXIT_CODE_RE = re.compile(r"^- EXIT_CODE: (-?[0-9]+)$")
_COUNT_RE = re.compile(r"^- (PASSED|FAILED|SKIPPED): ([0-9]+|n/a)$")
_COVERAGE_RE = re.compile(r"^- COVERAGE: (.+)$")
_LOG_RE = re.compile(r"^- LOG: `([^`]+)`$")
_LOG_HASH_RE = re.compile(r"^- LOG_SHA256: ([0-9a-f]{64})$")
_RESULT_RE = re.compile(r"^- RESULT: (.+)$")
_COVERAGE_VALUE_RE = re.compile(
    r"^statements=(\d+(?:\.\d+)?) branches=(\d+(?:\.\d+)?) "
    r"functions=(\d+(?:\.\d+)?) lines=(\d+(?:\.\d+)?)$"
)
_CONTRACT_GATE_RE = re.compile(r"(?m)^-\s+\*\*验收闸门\*\*[：:]\s*(.+?)\s*$")
_CODE_RE = re.compile(r"`([^`]+)`")


class EvidenceError(ValueError):
    """Gate evidence 或内容证据无效。"""


@dataclass(frozen=True)
class GateEvidence:
    """单条 task gate 的可复核证据。"""

    command: str
    owner: str
    level: str
    workdir: str
    kind: str
    exit_code: int
    passed: int | None
    failed: int | None
    skipped: int | None
    coverage: tuple[float, float, float, float] | None
    log: str
    log_sha256: str
    result: str


def _repo_path(value: str, field: str, *, allow_dot: bool = False) -> str:
    if allow_dot and value == ".":
        return value
    if "\\" in value or value.endswith("/"):
        raise EvidenceError(f"{field} 必须是规范仓库根相对路径：{value}")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or not path.parts
        or any(part in ("", ".", "..") for part in path.parts)
        or path.as_posix() != value
    ):
        raise EvidenceError(f"{field} 必须是规范仓库根相对路径：{value}")
    return value


def _count(line: str, label: str) -> int | None:
    match = _COUNT_RE.fullmatch(line)
    if match is None or match.group(1) != label:
        raise EvidenceError(f"Gate {label} 字段格式错误")
    return None if match.group(2) == "n/a" else int(match.group(2))


def _coverage(value: str) -> tuple[float, float, float, float] | None:
    if value == "n/a":
        return None
    match = _COVERAGE_VALUE_RE.fullmatch(value)
    if match is None:
        raise EvidenceError("COVERAGE 必须是 n/a 或四项固定 summary")
    metrics = tuple(float(item) for item in match.groups())
    if any(item < 0 or item > 100 for item in metrics):
        raise EvidenceError("COVERAGE 指标必须在 0..100")
    return metrics  # type: ignore[return-value]


def parse_gates(lines: list[str]) -> tuple[GateEvidence, ...]:
    """解析严格 Gate block；调用方负责先去除首尾空行。"""

    compact = [line for line in lines if line]
    if compact == [NONE]:
        return ()
    fields_per_gate = 14
    if len(compact) % fields_per_gate != 0:
        raise EvidenceError("每个 Gate 必须包含固定的 14 行字段")
    gates: list[GateEvidence] = []
    for offset in range(0, len(compact), fields_per_gate):
        block = compact[offset : offset + fields_per_gate]
        expected = len(gates) + 1
        header = _GATE_HEADER_RE.fullmatch(block[0])
        command = _COMMAND_RE.fullmatch(block[1])
        owner = _OWNER_RE.fullmatch(block[2])
        level = _LEVEL_RE.fullmatch(block[3])
        workdir = _WORKDIR_RE.fullmatch(block[4])
        kind = _KIND_RE.fullmatch(block[5])
        exit_code = _EXIT_CODE_RE.fullmatch(block[6])
        coverage = _COVERAGE_RE.fullmatch(block[10])
        log = _LOG_RE.fullmatch(block[11])
        log_hash = _LOG_HASH_RE.fullmatch(block[12])
        result = _RESULT_RE.fullmatch(block[13])
        if (
            header is None
            or int(header.group(1)) != expected
            or command is None
            or owner is None
            or level is None
            or workdir is None
            or kind is None
            or exit_code is None
            or coverage is None
            or log is None
            or log_hash is None
            or result is None
        ):
            raise EvidenceError(f"Gate {expected} 固定字段格式错误")
        if not command.group(1).strip() or not result.group(1).strip():
            raise EvidenceError(f"Gate {expected} 的 COMMAND/RESULT 不得为空")
        gate_kind = kind.group(1)
        if gate_kind not in GATE_KINDS:
            raise EvidenceError(f"Gate {expected} KIND 不合法：{gate_kind}")
        passed = _count(block[7], "PASSED")
        failed = _count(block[8], "FAILED")
        skipped = _count(block[9], "SKIPPED")
        coverage_values = _coverage(coverage.group(1))
        if gate_kind in {"test", "coverage"}:
            if passed is None or passed <= 0 or failed != 0 or skipped is None:
                raise EvidenceError(f"Gate {expected} 必须证明非零测试且 FAILED=0")
        elif any(value is not None for value in (passed, failed, skipped)):
            raise EvidenceError(f"Gate {expected} 非测试 gate 的计数必须为 n/a")
        if (gate_kind == "coverage") != (coverage_values is not None):
            raise EvidenceError(f"Gate {expected} coverage evidence 与 KIND 不一致")
        gates.append(
            GateEvidence(
                command=command.group(1),
                owner="task-agent",
                level="TASK_LOCAL",
                workdir=_repo_path(workdir.group(1), "WORKDIR", allow_dot=True),
                kind=gate_kind,
                exit_code=int(exit_code.group(1)),
                passed=passed,
                failed=failed,
                skipped=skipped,
                coverage=coverage_values,
                log=_repo_path(log.group(1), "LOG"),
                log_sha256=log_hash.group(1),
                result=result.group(1),
            )
        )
    return tuple(gates)


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def touched_content_hash(repo: Path, paths: tuple[str, ...]) -> str:
    """绑定 repo-relative 路径身份及当前 file/symlink/deleted 内容。"""

    digest = hashlib.sha256()
    for relative in sorted(paths):
        path = repo / relative
        if path.is_symlink():
            payload = b"symlink\0" + os.fsencode(os.readlink(path))
        elif path.is_file():
            payload = b"file\0" + path.read_bytes()
        elif not path.exists():
            payload = b"deleted\0"
        else:
            raise EvidenceError(f"Touched file 不是文件/symlink/deleted：{relative}")
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update(payload)
        digest.update(b"\0")
    return digest.hexdigest()


def _lint_consumable(root: Path, path: str) -> bool:
    """Return whether Bus can format this path without scanning another owner."""

    del root
    return Path(path).suffix == ".rs"


def existing_touched_files(repo: Path, paths: tuple[str, ...]) -> tuple[str, ...]:
    """返回 generation delta 中 scoped lint 可消费的现存文件。"""

    root = repo.resolve()
    return tuple(
        sorted(
            path
            for path in paths
            if ((root / path).is_file() or (root / path).is_symlink())
            and _lint_consumable(root, path)
        )
    )


def scoped_lint_command(paths: tuple[str, ...]) -> str:
    """生成 task runtime 唯一认可的 generation-delta scoped lint 命令。

    带 `--skip-typecheck`：类型依赖天然跨文件、跨 task owner，按文件范围裁剪不了。
    并行 task 共享同一棵树时，某个 task 的中间态必然让整树类型检查判红，
    task 的类型闭合由其自己的窄 typecheck 闸门负责，整树类型检查归 final lint。
    """

    if not paths:
        raise EvidenceError("scoped lint 至少需要一个 generation delta 中当前存在的文件")
    return shlex.join(("rustfmt", "--edition", "2021", "--check", *sorted(paths)))


def require_scoped_lint(
    paths: tuple[str, ...],
    gates: tuple[GateEvidence, ...],
) -> None:
    """完成态必须恰有一条绑定 generation delta 当前文件的只读 scoped lint。"""

    if not paths:
        return
    expected = scoped_lint_command(paths)
    matches = [
        gate
        for gate in gates
        if gate.command == expected
        and gate.owner == "task-agent"
        and gate.level == "TASK_LOCAL"
        and gate.workdir == "."
        and gate.kind == "check"
        and gate.exit_code == 0
    ]
    if len(matches) != 1:
        raise EvidenceError(
            "task report 必须恰有一条绑定 generation delta 当前文件的 scoped lint evidence："
            + expected
        )


def validate_artifacts(
    *,
    repo: Path,
    report_dir: Path,
    snapshot: Path,
    snapshot_sha256: str,
    touched_files: tuple[str, ...],
    touched_sha256: str,
    gates: tuple[GateEvidence, ...],
    require_contract_gates: bool = True,
) -> None:
    """验证 snapshot、内容、日志以及 task contract gate 命令。"""

    root = repo.resolve()
    if sha256_file(snapshot) != snapshot_sha256:
        raise EvidenceError("SNAPSHOT_SHA256 与 snapshot 内容不匹配")
    if touched_content_hash(root, touched_files) != touched_sha256:
        raise EvidenceError("TOUCHED_CONTENT_SHA256 与当前文件内容不匹配")
    # 只有完成态才承诺跑过契约闸门。OWNER_GAP / NEEDS_CONTEXT / BLOCKED 按定义是在跑闸门之前
    # 就停下的；在这些状态上仍强制契约命令，会让「必须先报 repair」的唯一出口本身不可达。
    if require_contract_gates:
        expected = _CODE_RE.findall(
            (_CONTRACT_GATE_RE.search(snapshot.read_text()) or _missing_gate()).group(1)
        )
        actual = [gate.command for gate in gates]
        missing = [command for command in expected if command not in actual]
        if missing:
            raise EvidenceError("Gate evidence 缺少任务契约命令：" + ", ".join(missing))
    for gate in gates:
        workdir = root if gate.workdir == "." else root / gate.workdir
        if not workdir.is_dir() or root not in (workdir.resolve(), *workdir.resolve().parents):
            raise EvidenceError(f"Gate WORKDIR 不存在或越出仓库：{gate.workdir}")
        log = root / gate.log
        if log.parent.resolve() != report_dir.resolve():
            raise EvidenceError(f"Gate LOG 必须位于当前 generation 目录：{gate.log}")
        if not log.is_file() or not log.read_bytes():
            raise EvidenceError(f"Gate LOG 不存在或为空：{gate.log}")
        if sha256_file(log) != gate.log_sha256:
            raise EvidenceError(f"Gate LOG_SHA256 漂移：{gate.log}")


def _missing_gate() -> re.Match[str]:
    raise EvidenceError("task snapshot 缺少单行验收闸门")
