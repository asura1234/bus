#!/usr/bin/env python3
"""delete-dead-code 的机械层：PR 模块范围、批次规划与产物校验。

这个 skill 唯一真正危险的失败不是漏删，而是**删到 PR 范围之外**。范围一旦交给自然语言约束，
带着全仓审计结论的 agent 就会往外飘——把范围推导、批次划分和产物校验都做成确定性脚本，
越界与自相矛盾才会变成非零退出，而不是一句被忽略的叮嘱。

模块的定义直接沿用仓库自己的定义（`scripts/lint/check-module-docs.mjs`）：**模块根 = 最近的
含 `AGENTS.md` 的祖先目录**。不另立一套路径分层表，否则 `shell/packages/video-editor/*` 这类
嵌套模块必然和仓库真实边界漂开。

范围有两个来源，互斥：默认从 `base...HEAD` 推导本次 PR 触及的模块；给了 `--directories` 就改为
在指定目录里找死代码（用于「这几棵树里扫一遍」这类明确请求）。两种模式共用同一套模块展开、
分批与产物校验——显式目录不是「放宽范围」，它只是换了一个同样机械的范围来源。

子命令：
  scope   --repo (--base B | --directories D…)            → JSON：模块、文件数、目录清单、excludes
  batches --repo (--base B | --directories D…) [--max-parallel N] → JSON：按文件数均衡的并行批次
  verify  --repo (--base B | --directories D…) --artifact → 范围校验 + 结构自洽校验
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path


DEFAULT_MAX_PARALLEL = 5
"""单轮并发上限。模块多于该值时按轮次串行，而不是一次全发出去。"""

MODULE_MARKER = "AGENTS.md"
"""模块根标记，与 `./run lint check` 的模块文档门禁同一定义。"""

REPOSITORY_MODULE = "<repository-root>"
"""顶层散文件（`.prettierrc.json` 等）的归属身份。它按定义不含任何目录。

不带 `AGENTS.md` 的顶层子树（`scripts/`、`cli_extensions/`、`cmake/`）**不**归这里：它们各自
成为一个单元。合成一个「仓库根」桶有两个后果，dogfood 第二轮两个都撞上了——改了
`cli_extensions/` 一个文件，agent 的地盘会连带 `scripts/`、`.github/`、`cmake/`（正是本 skill
最该防的越界形状）；而用「路径里没有 `/`」判成员资格，又会把 `cli_extensions/x.py` 判成越界。
"""

OUTCOMES = ("CLEAN", "ACTED", "REPORTED")
"""产物的三种终态。`REPORTED` 是「查到了真死代码但动不了」——缺了它，跨模块 finding 只能被塞进
`CLEAN`，汇总就会把有问题的模块报成干净的。dogfood 里真实发生过。"""

ACTING_DISPOSITIONS = ("DELETED", "CONSOLIDATED")

# 置信度只要求「以 `— ` 起头的一个字段」，不要求它后面紧跟分隔符：产物是手写的，agent 常在
# 该字段后补一句限定语（实测：`— CONFIRMED（生产侧无消费者）/ 测试侧为 observation seam —`）。
# 卡死尾随分隔符只会把合规内容判成格式错误，而真正要锁住的是「声明了哪一种置信度」。
_FINDING = re.compile(
    r"^\s*[-*]\s+`(?P<path>[^`:]+):(?P<line>\d+)`\s+—\s+"
    r"(?P<kind>DUPLICATE|DEAD-BRANCH|DEAD-CODE)\s+—.*?"
    r"—\s*(?P<confidence>CONFIRMED|LIKELY)\b"
)
_DISPOSITION = re.compile(
    r"^\s*[-*]\s+`(?P<path>[^`:]+):(?P<line>\d+)`\s+—\s+"
    r"(?P<disposition>DELETED|CONSOLIDATED|KEPT|HANDOFF)\b"
)
_ANY_ANCHOR = re.compile(r"^\s*[-*]\s+`(?P<path>[^`:]+):(?P<line>\d+)`")
_OUTCOME = re.compile(r"^\s*[-*]\s+Outcome:\s*`(?P<outcome>[A-Z]+)`\s*$")
_HEADING = re.compile(r"^##\s+(?P<title>.+?)\s*$")


class DeadCodeScopeError(Exception):
    """范围无法可信推导，或产物不可校验。"""


@dataclass(frozen=True)
class Module:
    name: str
    file_count: int
    excludes: tuple[str, ...] = field(default=())
    directories: tuple[str, ...] = field(default=())

    def as_json(self) -> dict[str, object]:
        # agent 的任务书必须逐字引用 excludes 与 directories，而不是自己从目录树重新推导一遍。
        return {
            "module": self.name,
            "fileCount": self.file_count,
            "excludes": list(self.excludes),
            "directories": list(self.directories),
        }


def _git(repository: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repository), *arguments],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise DeadCodeScopeError(
            f"git {' '.join(arguments)} 失败（{result.returncode}）：{result.stderr.strip()}"
        )
    return result.stdout


def changed_paths(repository: Path, base: str) -> list[str]:
    """PR 相对 base 的三点差异文件列表。"""
    output = _git(repository, "diff", "--name-only", f"{base}...HEAD")
    return sorted({line for line in output.splitlines() if line})


def tracked_paths(repository: Path, directories: list[str]) -> list[str]:
    """指定目录下的全部被跟踪文件。

    目录不存在是 fail-closed：拼错一个目录名而静默扫了个空，比报错糟得多——它会以「这里很干净」
    的形状返回，而没人能从输出上看出扫描根本没发生。
    """
    if not directories:
        raise DeadCodeScopeError("--directories 不能为空")
    paths: set[str] = set()
    for directory in directories:
        normalized = Path(directory).as_posix().rstrip("/")
        if not (repository / normalized).is_dir():
            raise DeadCodeScopeError(f"目录不存在：{directory}")
        entries = [
            line
            for line in _git(repository, "ls-files", "-z", "--", normalized).split("\0")
            if line
        ]
        if not entries:
            raise DeadCodeScopeError(f"目录里没有被跟踪文件：{directory}")
        paths.update(entries)
    return sorted(paths)


def module_of(repository: Path, path: str) -> str:
    """路径所属模块根：最近的含 `AGENTS.md` 的祖先目录。

    仓库根即使有 `AGENTS.md` 也不返回 `"."`：那个名字既不能和任何真实路径前缀匹配，又会读起来
    像「整仓都在范围内」。找不到模块标记时退到**顶层子树**（`scripts/lint/x.ts` → `scripts`），
    顶层散文件才归 `REPOSITORY_MODULE`，由 `_in_scope` 单独判定。
    """
    current = Path(path).parent
    while current != Path("."):
        if (repository / current / MODULE_MARKER).is_file():
            return current.as_posix()
        current = current.parent
    parts = Path(path).parts
    return parts[0] if len(parts) > 1 else REPOSITORY_MODULE


def nested_modules(repository: Path, name: str) -> tuple[str, ...]:
    """`name` 之下自带 `AGENTS.md` 的子目录——它们是别人的模块，不属于 `name`。

    这份清单必须由脚本给出：让每个 agent 自己从目录树推导「我的模块到底包含什么」，等于把一份
    机械事实抄进十几份任务书，抄错一次就是越权删除。
    """
    if name == REPOSITORY_MODULE:
        return ()
    root = repository / name
    if not root.is_dir():
        return ()
    nested = {
        marker.parent.relative_to(repository).as_posix()
        for marker in root.rglob(MODULE_MARKER)
        if marker.parent != root
    }
    return tuple(sorted(nested))


def _directories_of(entries: list[str], root: str) -> tuple[str, ...]:
    """`entries` 覆盖到的目录集合（含 `root` 自身）。"""
    directories = {root}
    for entry in entries:
        parent = Path(entry).parent.as_posix()
        while parent not in (root, ".", ""):
            directories.add(parent)
            parent = Path(parent).parent.as_posix()
    return tuple(sorted(directories))


def module_directories(
    repository: Path, name: str, excludes: tuple[str, ...] = ()
) -> tuple[str, ...]:
    """模块自己持有的目录：模块根，加上其下所有含被跟踪文件的子目录，剔除嵌套模块子树。

    目录从 `git ls-files` 推导，不从文件系统 walk：`node_modules/`、`build/`、`out/`、`dist/`
    都是未跟踪的，walk 会把它们一并写进任务书，agent 于是拿着一份大半是构建产物的地图去扫描。
    这与 guide.md 里「用 `git grep` 而不是 `grep -r`」是同一条理由——只不过那边的后果是漏删，
    这边的后果是把扫描预算烧在陈旧产物上。

    `REPOSITORY_MODULE` 没有目录：它按定义只持有顶层散文件，给它一个目录清单会读起来像「整仓」。
    """
    if name == REPOSITORY_MODULE:
        return ()
    excluded_prefixes = tuple(f"{item}/" for item in excludes)
    entries = [
        entry
        for entry in _git(repository, "ls-files", "-z", "--", name).split("\0")
        if entry and not entry.startswith(excluded_prefixes)
    ]
    return _directories_of(entries, name)


def scope(
    repository: Path, base: str | None = None, directories: list[str] | None = None
) -> list[Module]:
    """范围内的模块，按文件数降序、同数按名称升序。

    `directories` 为空时范围是本次 PR 触及的模块（`base...HEAD`）；给了 `directories` 就改为
    那些目录下的全部被跟踪文件所属的模块。两者都 fail-closed：范围为空时不应该有 agent 被发出去。
    """
    if directories:
        # 显式目录模式下**每个请求的目录就是一个单元**，不再展开成里面的嵌套模块。
        # 理由是「能不能动手」而不是「扫描面多大」：真实死代码的规范住所大多在兄弟模块——
        # base64 在 tools 想搬去 codecs、Windows 助手想搬去 process、canSplitClip 在 core
        # 想收敛到 model。只拥有一个叶子模块的 agent 对这些一律只能记 HANDOFF；拥有整棵树
        # 的 agent 直接就做了。粒度因此交给调用方：想切细就多传几个子目录。
        units: list[Module] = []
        for directory in directories:
            normalized = Path(directory).as_posix().rstrip("/")
            entries = tracked_paths(repository, [normalized])
            units.append(
                Module(
                    name=normalized,
                    file_count=len(entries),
                    excludes=(),
                    directories=_directories_of(entries, normalized),
                )
            )
        return sorted(units, key=lambda unit: (-unit.file_count, unit.name))
    else:
        if base is None:
            raise DeadCodeScopeError("需要 --base 或 --directories 之一")
        paths = changed_paths(repository, base)
        if not paths:
            raise DeadCodeScopeError(f"相对 {base} 没有变更文件，范围为空")
    counts: dict[str, int] = {}
    for path in paths:
        name = module_of(repository, path)
        counts[name] = counts.get(name, 0) + 1
    modules: list[Module] = []
    for name, count in sorted(counts.items(), key=lambda item: (-item[1], item[0])):
        excludes = nested_modules(repository, name)
        modules.append(
            Module(
                name=name,
                file_count=count,
                excludes=excludes,
                directories=module_directories(repository, name, excludes),
            )
        )
    return modules


def plan_batches(modules: list[Module], max_parallel: int) -> list[list[Module]]:
    """把模块摊进最少的轮次，并让各轮的扫描量尽量接近。

    轮次是串行的，一轮的耗时由该轮**最大**的模块决定，所以按文件数降序轮转发牌（而不是顺序切块）：
    大模块被摊开到不同轮次，避免某一轮里挤着两个巨型模块、另一轮全是小模块。
    """
    if max_parallel < 1:
        raise DeadCodeScopeError(f"--max-parallel 必须 >= 1，收到 {max_parallel}")
    if not modules:
        raise DeadCodeScopeError("没有模块可规划")
    round_count = -(-len(modules) // max_parallel)
    batches: list[list[Module]] = [[] for _ in range(round_count)]
    for index, module in enumerate(modules):
        batches[index % round_count].append(module)
    return batches


def _module_names(
    repository: Path, base: str | None, directories: list[str] | None
) -> set[str]:
    return {module.name for module in scope(repository, base, directories)}


def _in_scope(repository: Path, path: str, modules: set[str]) -> bool:
    if any(path == name or path.startswith(f"{name}/") for name in modules):
        return True
    # `REPOSITORY_MODULE` 只持有顶层散文件，所以「路径里没有 `/`」在这里是精确判定而非近似：
    # 带目录的路径一定归某个顶层子树，那是另一个模块名，已由上面的前缀匹配处理。
    return REPOSITORY_MODULE in modules and "/" not in path


def _sections(text: str) -> dict[str, list[str]]:
    """按 `## ` 标题切段。产物是 agent 手写的，缺段比乱序更常见，两者都要判得出来。"""
    sections: dict[str, list[str]] = {}
    current: str | None = None
    for line in text.splitlines():
        heading = _HEADING.match(line)
        if heading:
            current = heading.group("title")
            sections.setdefault(current, [])
            continue
        if current is not None:
            sections[current].append(line)
    return sections


def verify_artifact(
    repository: Path,
    base: str | None,
    artifact: Path,
    directories: list[str] | None = None,
) -> list[str]:
    """返回产物的全部问题；空列表表示通过。

    两类检查：**范围**（finding 路径必须落在本次 PR 的模块内）与**自洽**（Outcome、Findings、
    Disposition 三者不得互相矛盾）。只做范围检查不够——`CLEAN` 配 20 条 finding 一样会让汇总
    说谎，而那正是 dogfood 里真实发生过的事。
    """
    if not artifact.is_file():
        raise DeadCodeScopeError(f"产物不存在：{artifact}")
    text = artifact.read_text(encoding="utf-8")
    modules = _module_names(repository, base, directories)
    problems: list[str] = []

    outcome: str | None = None
    for line in text.splitlines():
        match = _OUTCOME.match(line)
        if match:
            outcome = match.group("outcome")
            break
    if outcome is None:
        problems.append(f"{artifact}：缺少 `- Outcome: ...` 行")
    elif outcome not in OUTCOMES:
        problems.append(f"{artifact}：未知 Outcome {outcome}，合法值 {'/'.join(OUTCOMES)}")

    sections = _sections(text)
    for required in ("Findings", "Disposition"):
        if required not in sections:
            problems.append(f"{artifact}：缺少 `## {required}` section")

    for number, line in enumerate(text.splitlines(), start=1):
        anchor = _ANY_ANCHOR.match(line)
        if anchor and not _in_scope(repository, anchor.group("path"), modules):
            problems.append(
                f"{artifact}:{number}：{anchor.group('path')} 不在本次 PR 的模块范围内"
            )

    findings: dict[str, str] = {}
    for line in sections.get("Findings", []):
        match = _FINDING.match(line)
        if match:
            findings[f"{match.group('path')}:{match.group('line')}"] = match.group("confidence")
        elif _ANY_ANCHOR.match(line):
            problems.append(f"{artifact}：finding 行不符合格式：{line.strip()[:80]}")

    dispositions: dict[str, str] = {}
    for line in sections.get("Disposition", []):
        match = _DISPOSITION.match(line)
        if match:
            dispositions[f"{match.group('path')}:{match.group('line')}"] = match.group(
                "disposition"
            )
        elif _ANY_ANCHOR.match(line):
            problems.append(f"{artifact}：disposition 行不符合格式：{line.strip()[:80]}")

    for anchor in sorted(set(findings) - set(dispositions)):
        problems.append(f"{artifact}：{anchor} 有 finding 但没有 disposition")
    for anchor in sorted(set(dispositions) - set(findings)):
        problems.append(f"{artifact}：{anchor} 有 disposition 但没有 finding")

    for anchor, disposition in sorted(dispositions.items()):
        if disposition in ACTING_DISPOSITIONS and findings.get(anchor) == "LIKELY":
            problems.append(
                f"{artifact}：{anchor} 标记 LIKELY 却被 {disposition}；未经确认不得删除"
            )

    acted = any(value in ACTING_DISPOSITIONS for value in dispositions.values())
    if outcome == "CLEAN" and findings:
        problems.append(
            f"{artifact}：Outcome 为 CLEAN 却有 {len(findings)} 条 finding；"
            "查到了但动不了应记为 REPORTED"
        )
    if outcome == "ACTED" and not acted:
        problems.append(f"{artifact}：Outcome 为 ACTED 但没有任何 DELETED/CONSOLIDATED")
    if outcome == "REPORTED" and acted:
        problems.append(
            f"{artifact}：Outcome 为 REPORTED 却存在 DELETED/CONSOLIDATED，应记为 ACTED"
        )
    if outcome == "REPORTED" and not findings:
        problems.append(f"{artifact}：Outcome 为 REPORTED 但没有任何 finding")
    return problems


_ASSERTION = re.compile(r"\b(?:expect|assert|EXPECT_[A-Z_]+|ASSERT_[A-Z_]+)\s*\(")
_TEST_PATH = re.compile(r"(?:^|/)(?:__tests__|tests?)/|(?:\.test\.[jt]sx?|_test\.(?:cc|cpp)|(?:^|/)test_[^/]+\.py)$")


def consolidations(artifacts: list[Path]) -> list[tuple[Path, str, str]]:
    """全部产物里 Disposition 为 CONSOLIDATED 的条目：`(产物, anchor, 理由)`。

    这些是主 agent **必须**逐条复核的对象。收敛是这个 skill 唯一能造成产品事故的动作——
    方向反了就是改线上行为，而且必然全绿，因为测试会被一起指到错的那份上。
    """
    found: list[tuple[Path, str, str]] = []
    for artifact in artifacts:
        for line in _sections(artifact.read_text(encoding="utf-8")).get("Disposition", []):
            match = _DISPOSITION.match(line)
            if match and match.group("disposition") == "CONSOLIDATED":
                anchor = f"{match.group('path')}:{match.group('line')}"
                found.append((artifact, anchor, line.strip()))
    return found


def rewritten_assertions(repository: Path, base: str) -> list[tuple[str, int]]:
    """测试文件里**被改写而不是被删掉**的断言行数。

    断言翻转是收敛方向出错的最强信号：留下的那份行为和生产原本的不一样时，唯一能让测试变绿的
    办法就是改期望。纯删除（僵尸测试整段移除）不算——那是正常处置。
    """
    diff = _git(repository, "diff", "-U0", base, "--", ".")
    current: str | None = None
    counts: dict[str, int] = {}
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            path = line[6:]
            current = path if _TEST_PATH.search(path) else None
            continue
        if current and line.startswith("+") and not line.startswith("+++"):
            if _ASSERTION.search(line):
                counts[current] = counts.get(current, 0) + 1
    return sorted(counts.items())


def handoffs(artifact: Path) -> list[str]:
    """产物里标为 HANDOFF 的 anchor：删除动作跨到了别的模块，需要主 agent 配对处置。"""
    text = artifact.read_text(encoding="utf-8")
    return [
        f"{match.group('path')}:{match.group('line')}"
        for line in _sections(text).get("Disposition", [])
        if (match := _DISPOSITION.match(line)) and match.group("disposition") == "HANDOFF"
    ]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("scope", "batches", "verify", "review"):
        child = sub.add_parser(name)
        child.add_argument("--repo", required=True, type=Path)
        source = child.add_mutually_exclusive_group(required=True)
        source.add_argument("--base")
        source.add_argument(
            "--directories",
            nargs="+",
            metavar="DIR",
            help="改为在这些目录里找死代码，而不是从 base...HEAD 推导 PR 范围。",
        )
        if name == "batches":
            child.add_argument("--max-parallel", type=int, default=DEFAULT_MAX_PARALLEL)
        if name == "verify":
            child.add_argument("--artifact", required=True, type=Path)
        if name == "review":
            child.add_argument("--artifact", required=True, nargs="+", type=Path)
    arguments = parser.parse_args(argv)

    try:
        if arguments.command == "scope":
            modules = scope(arguments.repo, arguments.base, arguments.directories)
            print(json.dumps([m.as_json() for m in modules], ensure_ascii=False, indent=2))
            return 0
        if arguments.command == "batches":
            modules = scope(arguments.repo, arguments.base, arguments.directories)
            batches = plan_batches(modules, arguments.max_parallel)
            print(
                json.dumps(
                    {
                        "roundCount": len(batches),
                        "rounds": [[m.as_json() for m in batch] for batch in batches],
                    },
                    ensure_ascii=False,
                    indent=2,
                )
            )
            return 0
        if arguments.command == "review":
            items = consolidations(list(arguments.artifact))
            rewritten = rewritten_assertions(arguments.repo, arguments.base or "HEAD")
            print(f"待复核的 CONSOLIDATED：{len(items)} 条")
            for artifact, anchor, line in items:
                print(f"  [{artifact.name}] {anchor}\n      {line}")
            print(f"被改写的测试断言：{sum(count for _, count in rewritten)} 行")
            for path, count in rewritten:
                print(f"  {path} (+{count})")
            if items or rewritten:
                print(
                    "\n主 agent 必须逐条走完 SKILL.md 的 REVIEW 阶段："
                    "从生产调用点往回读确认收敛方向，断言翻转必须有「按生产事实更新」的说明。",
                    file=sys.stderr,
                )
                return 1
            print("无需复核")
            return 0
        problems = verify_artifact(
            arguments.repo, arguments.base, arguments.artifact, arguments.directories
        )
        if problems:
            for problem in problems:
                print(problem, file=sys.stderr)
            return 1
        pending = handoffs(arguments.artifact)
        print("产物校验通过" + (f"；{len(pending)} 条 HANDOFF 待配对" if pending else ""))
        return 0
    except DeadCodeScopeError as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
