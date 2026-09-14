"""delete-dead-code 机械层的契约测试。

范围推导是这个 skill 唯一的安全边界，因此越界必须是硬失败而不是提示：正常、边界与
fail-closed 三条路径都在这里锁住。
"""

import subprocess
import sys
from pathlib import Path

import pytest


sys.path.insert(0, str(Path(__file__).resolve().parent))

from dead_code_scope import (  # noqa: E402
    REPOSITORY_MODULE,
    DeadCodeScopeError,
    Module,
    consolidations,
    handoffs,
    module_directories,
    module_of,
    nested_modules,
    plan_batches,
    rewritten_assertions,
    scope,
    tracked_paths,
    verify_artifact,
)


def _repo(tmp_path: Path) -> Path:
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    subprocess.run(["git", "-C", str(tmp_path), "config", "user.email", "t@t"], check=True)
    subprocess.run(["git", "-C", str(tmp_path), "config", "user.name", "t"], check=True)
    return tmp_path


def _commit(repository: Path, message: str) -> None:
    subprocess.run(["git", "-C", str(repository), "add", "-A"], check=True)
    subprocess.run(
        ["git", "-C", str(repository), "commit", "-q", "-m", message], check=True
    )


def _head(repository: Path) -> str:
    return subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def _write(repository: Path, relative: str, text: str = "x\n") -> None:
    target = repository / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


# --- module_of ---------------------------------------------------------------


def test_module_is_the_nearest_agents_md_ancestor(tmp_path: Path) -> None:
    _write(tmp_path, "shell/packages/video-editor/AGENTS.md")
    _write(tmp_path, "shell/packages/video-editor/react/AGENTS.md")
    # 最近的祖先胜出，嵌套模块不会被折叠到父模块。
    assert (
        module_of(tmp_path, "shell/packages/video-editor/react/src/a.ts")
        == "shell/packages/video-editor/react"
    )
    assert (
        module_of(tmp_path, "shell/packages/video-editor/other/b.ts")
        == "shell/packages/video-editor"
    )


def test_top_level_file_maps_to_repository_module_not_dot(tmp_path: Path) -> None:
    _write(tmp_path, "AGENTS.md")
    # `"."` 既匹配不上任何真实路径前缀，又读起来像「整仓都在范围内」。
    assert module_of(tmp_path, ".prettierrc.json") == REPOSITORY_MODULE


def test_path_without_any_agents_md_falls_back_to_its_top_level_subtree(
    tmp_path: Path,
) -> None:
    # `cli_extensions/`、`scripts/`、`cmake/` 都不带 `AGENTS.md`。把它们合成一个「仓库根」桶，
    # 改了其中一个文件就会让 agent 连带拥有另外两棵树——正是本 skill 最该防的越界形状。
    assert module_of(tmp_path, "cli_extensions/lint.py") == "cli_extensions"
    assert module_of(tmp_path, "scripts/lint/deep/file.ts") == "scripts"


def test_top_level_subtree_module_gets_its_own_directory_map(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "cli_extensions/lint.py")
    _write(repository, "scripts/lint/other.ts")
    _commit(repository, "base")
    # 只拿到自己那棵子树，不会把隔壁顶层目录一起收进地盘。
    assert module_directories(repository, "cli_extensions") == ("cli_extensions",)


# --- scope -------------------------------------------------------------------


def test_scope_counts_files_per_module_sorted_by_size(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "a/AGENTS.md")
    _write(repository, "b/AGENTS.md")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    _write(repository, "a/one.ts")
    _write(repository, "a/two.ts")
    _write(repository, "b/one.ts")
    _commit(repository, "change")
    assert scope(repository, base) == [
        Module(name="a", file_count=2, directories=("a",)),
        Module(name="b", file_count=1, directories=("b",)),
    ]


def test_empty_diff_fails_closed(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "seed.txt")
    _commit(repository, "base")
    # 没有变更就没有范围；此时发出 agent 只会让它去扫无关代码。
    with pytest.raises(DeadCodeScopeError, match="没有变更文件"):
        scope(repository, "HEAD")


def test_unknown_base_fails_closed(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "seed.txt")
    _commit(repository, "base")
    with pytest.raises(DeadCodeScopeError, match="git diff"):
        scope(repository, "no-such-ref")


# --- plan_batches ------------------------------------------------------------


def test_batches_use_the_fewest_rounds(tmp_path: Path) -> None:
    modules = [Module(name=f"m{i}", file_count=1) for i in range(12)]
    batches = plan_batches(modules, 5)
    assert len(batches) == 3
    assert sum(len(batch) for batch in batches) == 12
    assert all(len(batch) <= 5 for batch in batches)


def test_batches_spread_the_largest_modules_across_rounds(tmp_path: Path) -> None:
    # 轮次串行，一轮耗时由该轮最大模块决定：两个巨型模块必须落在不同轮次。
    modules = [
        Module(name="huge-a", file_count=100),
        Module(name="huge-b", file_count=90),
        Module(name="small-a", file_count=2),
        Module(name="small-b", file_count=1),
    ]
    batches = plan_batches(modules, 2)
    assert len(batches) == 2
    names_per_round = [{m.name for m in batch} for batch in batches]
    assert not any({"huge-a", "huge-b"} <= names for names in names_per_round)


def test_single_module_is_one_round(tmp_path: Path) -> None:
    assert plan_batches([Module(name="only", file_count=3)], 5) == [
        [Module(name="only", file_count=3)]
    ]


def test_zero_parallelism_fails_closed(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="必须 >= 1"):
        plan_batches([Module(name="m", file_count=1)], 0)


def test_no_modules_fails_closed(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="没有模块可规划"):
        plan_batches([], 5)


# --- verify_artifact ---------------------------------------------------------


def _scoped_repo(tmp_path: Path) -> tuple[Path, str]:
    repository = _repo(tmp_path)
    _write(repository, "in/AGENTS.md")
    _write(repository, "out/AGENTS.md")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = subprocess.run(
        ["git", "-C", str(repository), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    _write(repository, "in/touched.ts")
    _commit(repository, "change")
    return repository, base


def _artifact(repository: Path, outcome: str, findings: str, disposition: str) -> Path:
    path = repository / "findings.md"
    path.write_text(
        "# Dead Code Findings\n\n"
        "- Module: `in`\n"
        "- Base: `x`\n"
        f"- Outcome: `{outcome}`\n\n"
        f"## Findings\n\n{findings}\n\n"
        f"## Disposition\n\n{disposition}\n",
        encoding="utf-8",
    )
    return path


def test_finding_inside_scope_passes(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `in/touched.ts:12` — DEAD-CODE — `foo` — desc — CONFIRMED — grep",
        "- `in/touched.ts:12` — KEPT — seam",
    )
    assert verify_artifact(repository, base, artifact) == []


def test_finding_outside_scope_is_reported(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `out/untouched.ts:3` — DEAD-CODE — `bar` — desc — CONFIRMED — grep",
        "- `out/untouched.ts:3` — KEPT — seam",
    )
    offenders = [p for p in verify_artifact(repository, base, artifact) if "范围内" in p]
    assert len(offenders) == 2  # finding 行与 disposition 行各报一次
    assert all("out/untouched.ts" in offender for offender in offenders)


def test_module_prefix_is_not_matched_by_substring(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    # `in-other/` 不属于模块 `in`，前缀比较必须带分隔符。
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `in-other/x.ts:1` — DEAD-CODE — `baz` — desc — CONFIRMED — grep",
        "- `in-other/x.ts:1` — KEPT — seam",
    )
    assert [p for p in verify_artifact(repository, base, artifact) if "范围内" in p]


def test_prose_lines_are_ignored(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    # 正文散文提到某个越界路径不算 finding，不该触发范围报错。
    artifact = _artifact(
        repository,
        "CLEAN",
        "- 无\n\n本轮没有发现越界项，正文里提到 out/untouched.ts 也不算 finding。",
        "- 无",
    )
    assert verify_artifact(repository, base, artifact) == []


def test_missing_artifact_fails_closed(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    with pytest.raises(DeadCodeScopeError, match="产物不存在"):
        verify_artifact(repository, base, repository / "absent.md")


# --- excludes (nested modules) ----------------------------------------------


def test_scope_reports_nested_modules_as_excludes(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/AGENTS.md")
    _write(repository, "parent/child/AGENTS.md")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "parent/own.ts")
    _commit(repository, "change")
    # 嵌套模块必须由脚本报出来，而不是让每个 agent 自己从目录树推一遍。
    assert scope(repository, base)[0].excludes == ("parent/child",)


def test_repository_module_has_no_excludes(tmp_path: Path) -> None:
    assert nested_modules(tmp_path, REPOSITORY_MODULE) == ()


# --- directories (模块自己的目录地图) ------------------------------------------


def test_directories_list_module_root_and_tracked_subdirectories(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _write(repository, "m/src/a.ts")
    _write(repository, "m/src/deep/b.ts")
    _write(repository, "m/tests/c.ts")
    _commit(repository, "base")
    assert module_directories(repository, "m") == (
        "m",
        "m/src",
        "m/src/deep",
        "m/tests",
    )


def test_directories_exclude_nested_module_subtrees(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/AGENTS.md")
    _write(repository, "parent/own/a.ts")
    _write(repository, "parent/child/AGENTS.md")
    _write(repository, "parent/child/deep/b.ts")
    _commit(repository, "base")
    # 嵌套模块是别人的地盘：它既在 excludes 里，也不能出现在我的目录地图上。
    assert module_directories(repository, "parent", ("parent/child",)) == (
        "parent",
        "parent/own",
    )


def test_directories_ignore_untracked_build_output(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, ".gitignore", "node_modules/\ndist/\n")
    _write(repository, "m/AGENTS.md")
    _write(repository, "m/src/a.ts")
    _commit(repository, "base")
    _write(repository, "m/node_modules/pkg/index.js")
    _write(repository, "m/dist/bundle.js")
    # 目录地图来自 git ls-files：构建产物不该占掉 agent 的扫描预算。
    assert module_directories(repository, "m") == ("m", "m/src")


def test_repository_module_has_no_directories(tmp_path: Path) -> None:
    assert module_directories(tmp_path, REPOSITORY_MODULE) == ()


def test_scope_attaches_directories_to_each_module(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _write(repository, "m/src/a.ts")
    _write(repository, "seed.txt")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "m/src/b.ts")
    _commit(repository, "change")
    assert scope(repository, base)[0].directories == ("m", "m/src")


# --- explicit --directories scope --------------------------------------------


def test_directories_scope_covers_every_tracked_file_not_just_changed_ones(
    tmp_path: Path,
) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _write(repository, "m/src/a.ts")
    _write(repository, "m/src/b.ts")
    _write(repository, "other/AGENTS.md")
    _write(repository, "other/c.ts")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "m/src/a.ts", "changed\n")
    _commit(repository, "change")
    # PR 范围只看改过的文件；显式目录看整棵树，这正是「这几棵树里扫一遍」需要的语义。
    assert [m.name for m in scope(repository, base)] == ["m"]
    assert scope(repository, base)[0].file_count == 1
    explicit = scope(repository, directories=["m"])
    assert [m.name for m in explicit] == ["m"]
    assert explicit[0].file_count == 3  # AGENTS.md + 两个源文件


def test_directories_scope_treats_each_directory_as_one_unit(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/AGENTS.md")
    _write(repository, "parent/own.ts")
    _write(repository, "parent/child/AGENTS.md")
    _write(repository, "parent/child/deep.ts")
    _commit(repository, "base")
    # 请求一棵树就得到**一个**单元，嵌套模块不再拆出来。判据是「能不能动手」：规范住所大多在
    # 兄弟模块，只拥有一个叶子的 agent 对这类收敛一律只能记 HANDOFF。
    units = scope(repository, directories=["parent"])
    assert [unit.name for unit in units] == ["parent"]
    assert units[0].file_count == 4
    assert units[0].excludes == ()
    assert units[0].directories == ("parent", "parent/child")


def test_directories_scope_granularity_is_the_callers_choice(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "parent/AGENTS.md")
    _write(repository, "parent/own.ts")
    _write(repository, "parent/child/AGENTS.md")
    _write(repository, "parent/child/deep.ts")
    _commit(repository, "base")
    # 想切细就多传几个子目录——粒度由调用方控制，不由脚本替他决定。
    units = scope(repository, directories=["parent/child"])
    assert [unit.name for unit in units] == ["parent/child"]
    assert units[0].file_count == 2


def test_missing_directory_fails_closed(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _commit(repository, "base")
    # 拼错目录名而静默扫了个空，会以「这里很干净」的形状返回，没人看得出扫描没发生。
    with pytest.raises(DeadCodeScopeError, match="目录不存在"):
        scope(repository, directories=["m", "typo"])


def test_directory_without_tracked_files_fails_closed(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "m/AGENTS.md")
    _commit(repository, "base")
    (repository / "empty").mkdir()
    with pytest.raises(DeadCodeScopeError, match="没有被跟踪文件"):
        tracked_paths(repository, ["empty"])


def test_empty_directory_list_fails_closed(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="不能为空"):
        tracked_paths(tmp_path, [])


def test_scope_requires_a_range_source(tmp_path: Path) -> None:
    with pytest.raises(DeadCodeScopeError, match="--base 或 --directories"):
        scope(tmp_path)


def test_verify_uses_the_directories_scope_when_given(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "in/AGENTS.md")
    _write(repository, "in/a.ts")
    _write(repository, "out/AGENTS.md")
    _write(repository, "out/b.ts")
    _commit(repository, "base")
    artifact = _artifact(repository, "ACTED", _FOUND, "- `in/a.ts:1` — DELETED — 无消费者")
    assert verify_artifact(repository, None, artifact, ["in"]) == []
    # `out/` 没被请求，它就是范围外——显式目录一样受越界校验约束。
    artifact = _artifact(
        repository,
        "ACTED",
        "- `out/b.ts:1` — DEAD-CODE — `foo` — desc — CONFIRMED — git grep",
        "- `out/b.ts:1` — DELETED — 无消费者",
    )
    problems = verify_artifact(repository, None, artifact, ["in"])
    assert any("不在本次 PR 的模块范围内" in problem for problem in problems)


# --- artifact structural checks ---------------------------------------------


_FOUND = "- `in/a.ts:1` — DEAD-CODE — `foo` — desc — CONFIRMED — git grep"
_FOUND_LIKELY = "- `in/a.ts:1` — DEAD-CODE — `foo` — desc — LIKELY — git grep"


def test_clean_with_findings_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "CLEAN", _FOUND, "- `in/a.ts:1` — KEPT — reason")
    problems = verify_artifact(repository, base, artifact)
    # dogfood 里真实发生过：查到十条却记成 CLEAN，汇总就把有问题的模块报成干净的。
    assert any("应记为 REPORTED" in p for p in problems)


def test_reported_with_findings_and_no_action_passes(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository, "REPORTED", _FOUND, "- `in/a.ts:1` — HANDOFF — 阻塞在 out 模块"
    )
    assert verify_artifact(repository, base, artifact) == []


def test_acted_without_any_action_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "ACTED", _FOUND, "- `in/a.ts:1` — KEPT — reason")
    assert any("没有任何 DELETED" in p for p in verify_artifact(repository, base, artifact))


def test_reported_that_actually_deleted_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "REPORTED", _FOUND, "- `in/a.ts:1` — DELETED — gone")
    assert any("应记为 ACTED" in p for p in verify_artifact(repository, base, artifact))


def test_likely_may_not_be_deleted(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository, "ACTED", _FOUND_LIKELY, "- `in/a.ts:1` — DELETED — gone"
    )
    assert any("未经确认不得删除" in p for p in verify_artifact(repository, base, artifact))


def test_finding_without_disposition_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "REPORTED", _FOUND, "- 无")
    assert any("没有 disposition" in p for p in verify_artifact(repository, base, artifact))


def test_disposition_without_finding_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(repository, "CLEAN", "- 无", "- `in/a.ts:1` — KEPT — reason")
    assert any("没有 finding" in p for p in verify_artifact(repository, base, artifact))


def test_missing_outcome_line_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = repository / "findings.md"
    artifact.write_text("# Dead Code Findings\n\n## Findings\n\n- 无\n\n## Disposition\n\n- 无\n", encoding="utf-8")
    assert any("缺少" in p and "Outcome" in p for p in verify_artifact(repository, base, artifact))


def test_missing_section_is_rejected(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = repository / "findings.md"
    artifact.write_text(
        "# Dead Code Findings\n\n- Outcome: `CLEAN`\n\n## Findings\n\n- 无\n", encoding="utf-8"
    )
    assert any("缺少 `## Disposition`" in p for p in verify_artifact(repository, base, artifact))


def test_confidence_may_carry_a_trailing_qualifier(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    # 实测产物写成 `— CONFIRMED（生产侧无消费者）/ 测试侧为 observation seam —`；
    # 卡死尾随分隔符会把合规内容判成格式错误。
    artifact = _artifact(
        repository,
        "REPORTED",
        "- `in/a.ts:1` — DEAD-CODE — `foo` — desc — CONFIRMED（仅生产侧）/ 测试仍读 — git grep",
        "- `in/a.ts:1` — KEPT — observation seam",
    )
    assert verify_artifact(repository, base, artifact) == []


def test_handoffs_lists_only_handoff_anchors(tmp_path: Path) -> None:
    repository, base = _scoped_repo(tmp_path)
    artifact = _artifact(
        repository,
        "REPORTED",
        _FOUND + "\n- `in/b.ts:2` — DEAD-CODE — `bar` — desc — CONFIRMED — git grep",
        "- `in/a.ts:1` — HANDOFF — 阻塞在 out 模块\n- `in/b.ts:2` — KEPT — seam",
    )
    assert verify_artifact(repository, base, artifact) == []
    assert handoffs(artifact) == ["in/a.ts:1"]


# --- main-agent REVIEW 的机械枚举 -------------------------------------------


def test_consolidations_lists_only_consolidated_anchors(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    artifact = _artifact(
        repository,
        "ACTED",
        f"{_FOUND}\n- `in/b.ts:2` — DUPLICATE — `bar` — d — CONFIRMED — g",
        "- `in/a.ts:1` — DELETED — 无消费者\n- `in/b.ts:2` — CONSOLIDATED — 迁到规范实现",
    )
    # 只有收敛需要主 agent 逐条复核：纯删除不会改行为，方向反了的收敛会。
    assert [anchor for _, anchor, _ in consolidations([artifact])] == ["in/b.ts:2"]


def test_rewritten_assertions_flags_changed_but_not_deleted_expectations(
    tmp_path: Path,
) -> None:
    repository = _repo(tmp_path)
    _write(repository, "src/__tests__/a.test.ts", "expect(one).toBe(1)\n")
    _write(repository, "src/prod.ts", "export const one = 1\n")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "src/__tests__/a.test.ts", "expect(one).toBe(2)\n")
    _commit(repository, "flip")
    # 断言被改写是收敛方向出错的最强信号——留下的那份行为不同，唯一变绿的办法就是改期望。
    assert rewritten_assertions(repository, base) == [("src/__tests__/a.test.ts", 1)]


def test_deleting_a_zombie_test_is_not_a_rewritten_assertion(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "src/__tests__/a.test.ts", "expect(one).toBe(1)\n")
    _commit(repository, "base")
    base = _head(repository)
    (repository / "src/__tests__/a.test.ts").unlink()
    _commit(repository, "remove zombie")
    # 僵尸测试整段删除是正常处置，不该把主 agent 的复核清单撑满。
    assert rewritten_assertions(repository, base) == []


def test_production_source_changes_are_not_counted_as_assertions(tmp_path: Path) -> None:
    repository = _repo(tmp_path)
    _write(repository, "src/prod.ts", "const x = 1\n")
    _commit(repository, "base")
    base = _head(repository)
    _write(repository, "src/prod.ts", "expect(x).toBe(1)\n")
    _commit(repository, "change")
    assert rewritten_assertions(repository, base) == []
