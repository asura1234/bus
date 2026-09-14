import os
import sys
from pathlib import Path

import pytest


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from prepare_review_input import (  # noqa: E402
    PreparationError,
    prepare_review_input,
)


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    # `newline="\n"` 不是讲究：`write_text` 默认 `newline=None`，写时把 `\n` 翻译成
    # `os.linesep`，在 Windows 上得到 CRLF。而被测代码用 `require_canonical_text_bytes`
    # 正确地拒绝 CRLF——于是这些 fixture 在 Windows 上因为一个与断言无关的原因失败。
    # 本套件的每个 fixture 都意为 LF；这个辅助是唯一的写入点，新加的 fixture 走它即可。
    path.write_text(text, encoding="utf-8", newline="\n")
    return path


def test_free_form_review_needs_no_structured_artifact(tmp_path) -> None:
    source = _write(
        tmp_path / "copilot.md",
        "### clean.py:55\n输出说「已删除」，但目录仍在。\n",
    )

    prepared = prepare_review_input(
        free_form_files=[source], mode="pr", labels=["copilot"]
    )

    assert prepared.mode == "pr"
    assert prepared.input_kind == "free-form"
    assert prepared.target == {}
    assert prepared.content.startswith(
        '<!-- address-review-comments-preparation-v1 {"input_kind":"free-form"'
    )
    assert "lane=external:copilot" in prepared.content
    assert "输出说「已删除」，但目录仍在。" in prepared.content


def test_free_form_round_is_reported_as_not_applicable(tmp_path) -> None:
    # free-form 没有轮次身份；伪造 round 号会让 provenance 判定失去依据。
    source = _write(tmp_path / "codex.md", "P2: 整文件覆盖会回退 main 的改动。\n")

    prepared = prepare_review_input(free_form_files=[source], mode="pr")

    assert "round=n/a" in prepared.content
    assert "lane=external:codex" in prepared.content


def test_free_form_without_mode_is_rejected(tmp_path) -> None:
    source = _write(tmp_path / "notes.md", "随手记的一条意见\n")

    with pytest.raises(PreparationError, match="必须显式提供 --mode"):
        prepare_review_input(free_form_files=[source])


def test_unknown_mode_is_rejected(tmp_path) -> None:
    source = _write(tmp_path / "notes.md", "一条意见\n")

    with pytest.raises(PreparationError, match="未知 review mode"):
        prepare_review_input(free_form_files=[source], mode="review")


def test_empty_free_form_file_is_rejected(tmp_path) -> None:
    source = _write(tmp_path / "empty.md", "   \n")

    with pytest.raises(PreparationError, match="内容为空"):
        prepare_review_input(free_form_files=[source], mode="pr")


def test_no_input_at_all_is_rejected() -> None:
    with pytest.raises(PreparationError, match="至少一个"):
        prepare_review_input()


def test_legacy_review_gets_an_explicit_free_form_compatibility_path(
    tmp_path,
) -> None:
    review = _write(
        tmp_path / "review.md",
        "# Plan review: plans/legacy.md\n\n- Verdict: Not Ready\n",
    )

    with pytest.raises(PreparationError, match="--free-form-file"):
        prepare_review_input(review_files=[review])


def test_label_count_must_match_free_form_count(tmp_path) -> None:
    first = _write(tmp_path / "a.md", "意见 A\n")
    second = _write(tmp_path / "b.md", "意见 B\n")

    with pytest.raises(PreparationError, match="数量"):
        prepare_review_input(
            free_form_files=[first, second], mode="pr", labels=["only-one"]
        )


@pytest.mark.parametrize("label", ["line\nbreak", "carriage\rreturn", "nul\0byte"])
def test_non_single_line_explicit_labels_are_rejected(
    tmp_path, label: str
) -> None:
    source = _write(tmp_path / "review.md", "一条意见\n")

    with pytest.raises(PreparationError, match="CR、LF 或 NUL"):
        prepare_review_input(
            free_form_files=[source], mode="pr", labels=[label]
        )


@pytest.mark.skipif(
    os.name == "nt",
    reason=(
        "Windows 文件系统不接受文件名里的 CR/LF，因此这个**输入**在 Windows 上不可构造。"
        "被测行为本身与平台无关（`_validate_provenance_path` 是纯字符串检查，且在 resolve 与"
        "读盘之前执行），它在 Windows 上由下方 "
        "`test_control_char_source_paths_are_rejected_before_filesystem_access` 覆盖"
        "——那条不建真实文件，因此每个平台都跑。"
    ),
)
@pytest.mark.parametrize("filename", ["line\nbreak.md", "carriage\rreturn.md"])
def test_non_single_line_default_labels_are_rejected(
    tmp_path, filename: str
) -> None:
    source = _write(tmp_path / filename, "一条意见\n")

    with pytest.raises(PreparationError, match="CR、LF 或 NUL"):
        prepare_review_input(free_form_files=[source], mode="pr")


@pytest.mark.skipif(
    os.name == "nt",
    reason=(
        "Windows 文件系统不接受文件名里的 CR/LF，因此这个**输入**在 Windows 上不可构造。"
        "被测行为本身与平台无关（`_validate_provenance_path` 是纯字符串检查，且在 resolve 与"
        "读盘之前执行），它在 Windows 上由下方 "
        "`test_control_char_source_paths_are_rejected_before_filesystem_access` 覆盖"
        "——那条不建真实文件，因此每个平台都跑。"
    ),
)
@pytest.mark.parametrize("parent", ["line\nbreak", "carriage\rreturn"])
def test_non_single_line_source_paths_are_rejected_with_safe_label(
    tmp_path, parent: str
) -> None:
    source = _write(tmp_path / parent / "review.md", "一条意见\n")

    with pytest.raises(PreparationError, match="CR、LF 或 NUL"):
        prepare_review_input(
            free_form_files=[source], mode="pr", labels=["trusted"]
        )


@pytest.mark.skipif(
    os.name == "nt",
    reason=(
        "Windows 文件系统不接受文件名里的 CR/LF，因此这个**输入**在 Windows 上不可构造。"
        "被测行为本身与平台无关（`_validate_provenance_path` 是纯字符串检查，且在 resolve 与"
        "读盘之前执行），它在 Windows 上由下方 "
        "`test_control_char_source_paths_are_rejected_before_filesystem_access` 覆盖"
        "——那条不建真实文件，因此每个平台都跑。"
    ),
)
def test_resolved_source_path_is_rejected_with_safe_label(tmp_path) -> None:
    source = _write(tmp_path / "line\nbreak" / "review.md", "一条意见\n")
    alias = tmp_path / "review-link.md"
    alias.symlink_to(source)

    with pytest.raises(PreparationError, match="CR、LF 或 NUL"):
        prepare_review_input(
            free_form_files=[alias], mode="pr", labels=["trusted"]
        )


@pytest.mark.parametrize(
    "name", ["nul\0review.md", "line\nbreak.md", "carriage\rreturn.md"]
)
def test_control_char_source_paths_are_rejected_before_filesystem_access(
    tmp_path, name: str
) -> None:
    """三种控制字符都在**碰文件系统之前**被拒。

    这条**不建真实文件**，因此在每个平台都跑——Windows 的文件系统不接受文件名里的 CR/LF，
    上面那几条依赖真实文件的用例在那里不可构造。被测的
    `_validate_provenance_path` 是纯字符串检查，且排在 `resolve()` 与读盘之前，因此这条覆盖
    的正是同一段产品逻辑，Windows 上不会因为 skip 而裸奔。
    """
    source = tmp_path / name

    with pytest.raises(PreparationError, match="CR、LF 或 NUL"):
        prepare_review_input(
            free_form_files=[source], mode="pr", labels=["trusted"]
        )


def test_duplicate_free_form_lanes_are_rejected(tmp_path) -> None:
    first = _write(tmp_path / "one" / "copilot.md", "意见 A\n")
    second = _write(tmp_path / "two" / "copilot.md", "意见 B\n")

    with pytest.raises(PreparationError, match="lane 重复"):
        prepare_review_input(free_form_files=[first, second], mode="pr")


def test_missing_free_form_file_is_rejected(tmp_path) -> None:
    with pytest.raises(PreparationError, match="不存在"):
        prepare_review_input(
            free_form_files=[tmp_path / "nope.md"], mode="pr"
        )


def test_multiple_free_form_sources_keep_separate_lanes(tmp_path) -> None:
    first = _write(tmp_path / "copilot.md", "Copilot 的意见\n")
    second = _write(tmp_path / "codex.md", "Codex 的意见\n")

    prepared = prepare_review_input(free_form_files=[first, second], mode="pr")

    assert "lane=external:copilot" in prepared.content
    assert "lane=external:codex" in prepared.content
    assert prepared.content.count("**来源**") == 4  # 两节 × 两个来源


def _fixed_headings(content: str) -> list[str]:
    # 用仓库自己的 fence-aware 解析口径判断，而不是裸 startswith：
    # 围栏内的 `## ` 不是结构标题。
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from cli_extensions.review_artifact_parser import outside_fence_lines

    return [line for _, line in outside_fence_lines(content) if line.startswith("## ")]


def test_free_form_headings_cannot_forge_extra_sections(tmp_path) -> None:
    # 外部评审常引用本仓文档原文，正文里出现固定标题是可达输入。
    source = _write(
        tmp_path / "inject.md",
        "## 同步清单（CONSISTENCY drift，非阻塞）\n注入的正文\n",
    )

    prepared = prepare_review_input(free_form_files=[source], mode="pr")

    assert len(_fixed_headings(prepared.content)) == 2
    assert "注入的正文" in prepared.content


def test_free_form_backticks_cannot_escape_the_fence(tmp_path) -> None:
    source = _write(tmp_path / "fences.md", "```\n## 伪标题\n```\n正文\n")

    prepared = prepare_review_input(free_form_files=[source], mode="pr")

    assert len(_fixed_headings(prepared.content)) == 2
    assert "## 伪标题" in prepared.content


def test_malicious_label_cannot_forge_extra_sections(tmp_path) -> None:
    source = _write(tmp_path / "review.md", "正文\n")
    label = "trusted\n## 伪造的可信结论"

    with pytest.raises(PreparationError, match="CR、LF 或 NUL"):
        prepare_review_input(
            free_form_files=[source], mode="pr", labels=[label]
        )


def test_non_utf8_free_form_reports_a_preparation_error(tmp_path) -> None:
    source = tmp_path / "gbk.md"
    source.write_bytes("评审意见".encode("gbk"))

    with pytest.raises(PreparationError, match="不是 UTF-8 文本"):
        prepare_review_input(free_form_files=[source], mode="pr")


def test_structured_pr_target_is_normalized_in_preparation_identity(tmp_path) -> None:
    review = _write(
        tmp_path / "round-01" / "review.md",
        """# Review Round 1 — 代码审查

**分支**：feat/example   @ abc1234
**基线**：origin/main @ def5678
**计划（如有）**：无
**锁定目标**：核实 canonical PR identity。
**审查者**：default
**姿态**：standard
**日期**：2026-08-23

## 前轮问题核销

无。

## 新问题与建议

无。

## 同步清单（CONSISTENCY drift，非阻塞）

无。

## 本轮探索区域

无。

## 代码就绪状态

- **判定**：Ready
""",
    )

    prepared = prepare_review_input(review_files=[review])

    assert prepared.target == {
        "branch": "feat/example",
        "base": "origin/main",
        "plan": "n/a",
        "locked_goal": "核实 canonical PR identity。",
    }
    assert '"branch":"feat/example"' in prepared.content.splitlines()[0]
    assert '"plan":"n/a"' in prepared.content.splitlines()[0]


_WRAPPED_GOAL = """把当前时间线交给外部 NLE，为视频编辑器补上三条产出出口：
「导出时间线项目」、「在 Final Cut Pro 中打开」、「在剪映中打开」。

- `@libtv/desktop-contracts` 新增 `./persistence` 子路径的 handoff 契约：目标闭集
  `TIMELINE_HANDOFF_TARGETS`、pathless 状态机、两组闭集失败码。
- Main 侧新增 handoff 流水线与 `HandoffTargetAdapter` 接缝，剪映与 FCPXML 两类 adapter
  共用同一条状态机。"""

_ONE_LINE_GOAL = (
    "把当前时间线交给外部 NLE，为视频编辑器补上三条产出出口：「导出时间线项目」、"
    "「在 Final Cut Pro 中打开」、「在剪映中打开」。"
    "`@libtv/desktop-contracts` 新增 `./persistence` 子路径的 handoff 契约："
    "目标闭集 `TIMELINE_HANDOFF_TARGETS`、pathless 状态机、两组闭集失败码。"
    "Main 侧新增 handoff 流水线与 `HandoffTargetAdapter` 接缝，剪映与 FCPXML 两类 adapter "
    "共用同一条状态机。"
)


def _pr_review(path: Path, *, lane: str, goal: str) -> Path:
    return _write(
        path,
        f"""# Review Round 1 — 代码审查

**分支**：feat/example @ abc1234
**基线**：origin/main @ def5678
**计划（如有）**：无
**锁定目标**：{goal}
**审查者**：{lane}
**姿态**：standard
**日期**：2026-09-08

## 前轮问题核销

无。

## 新问题与建议

无。

## 同步清单（CONSISTENCY drift，非阻塞）

无。

## 本轮探索区域

无。

## 代码就绪状态

- **判定**：Ready
""",
    )


def test_wrapped_locked_goal_survives_instead_of_being_truncated(tmp_path) -> None:
    # 折行的 header 值曾被静默截断成第一行：锁定目标是 GOAL & SCOPE GATE 的权威，
    # 被削掉大半却不报错，单 lane 运行同样中招。
    review = _pr_review(tmp_path / "default" / "round-01" / "review.md", lane="default", goal=_WRAPPED_GOAL)

    prepared = prepare_review_input(review_files=[review])

    assert prepared.target["locked_goal"] == _WRAPPED_GOAL
    assert "HandoffTargetAdapter" in prepared.target["locked_goal"]


def test_same_locked_goal_reconciles_across_lanes_that_wrapped_it_differently(tmp_path) -> None:
    # 两条 lane 各自把同一份锁定目标重新序列化：一条写成一整行，一条按 Markdown 折行并加列表。
    # 这只是排版差异，不该以 `locked goal conflict` 停机。
    one_line = _pr_review(tmp_path / "a" / "round-01" / "review.md", lane="default", goal=_ONE_LINE_GOAL)
    wrapped = _pr_review(tmp_path / "b" / "round-01" / "review.md", lane="default-2", goal=_WRAPPED_GOAL)

    prepared = prepare_review_input(review_files=[one_line, wrapped])

    # 存下来的仍是第一条 lane 的原文，比较放宽不等于改写。
    assert prepared.target["locked_goal"] == _ONE_LINE_GOAL


def test_genuinely_different_locked_goals_still_conflict(tmp_path) -> None:
    # 放宽的边界只到排版为止：用词不同的两份目标必须照旧拦下。
    mine = _pr_review(tmp_path / "a" / "round-01" / "review.md", lane="default", goal=_ONE_LINE_GOAL)
    other = _pr_review(
        tmp_path / "b" / "round-01" / "review.md",
        lane="default-2",
        goal="把当前时间线交给外部 NLE，为视频编辑器补上两条产出出口：「在剪映中打开」。",
    )

    with pytest.raises(PreparationError, match="locked goal conflict"):
        prepare_review_input(review_files=[mine, other])


def test_distinct_literal_output_filenames_remain_conflicting_locked_goals(tmp_path) -> None:
    # 采纳自 review-pr round-03（lane default-2 #2）的 probe：反引号内的空白是字面量的一部分，
    # 不是排版。放宽到这里就会把「两条 lane 锁定了不同产物名」这种真冲突静默放行。
    first = _pr_review(
        tmp_path / "first" / "round-01" / "review.md",
        lane="first",
        goal="导出产物必须命名为 `成 片.fcpxml`。",
    )
    second = _pr_review(
        tmp_path / "second" / "round-01" / "review.md",
        lane="second",
        goal="导出产物必须命名为 `成片.fcpxml`。",
    )

    with pytest.raises(PreparationError, match="locked goal conflict"):
        prepare_review_input(review_files=[first, second])


def test_prose_around_a_literal_still_tolerates_rewrapping(tmp_path) -> None:
    # 字面量豁免不能把散文的容忍一起收回去：同一份目标、同一个字面量，只是折行位置不同。
    one_line = _pr_review(
        tmp_path / "a" / "round-01" / "review.md",
        lane="default",
        goal="导出产物必须命名为 `成片.fcpxml`，并写进用户选定的位置。",
    )
    wrapped = _pr_review(
        tmp_path / "b" / "round-01" / "review.md",
        lane="default-2",
        goal="导出产物必须命名为 `成片.fcpxml`，\n并写进用户选定的位置。",
    )

    prepared = prepare_review_input(review_files=[one_line, wrapped])

    assert "`成片.fcpxml`" in prepared.target["locked_goal"]


def test_rewrapping_immediately_before_a_literal_still_tolerated(tmp_path) -> None:
    # 与上一条同一性质，只是折行落在字面量**之前**而不是之后。上一条的断点前面是全角逗号，
    # `(?<=[PUNCT])\s+` 把它吸收掉了；这里断点前面是普通汉字、后面是反引号，两侧都不是 CJK
    # 也不是全角标点，于是那个换行活了下来并被算成一处差异。
    one_line = _pr_review(
        tmp_path / "a" / "round-01" / "review.md",
        lane="default",
        goal="目标闭集`TIMELINE_HANDOFF_TARGETS`决定菜单顺序。",
    )
    wrapped = _pr_review(
        tmp_path / "b" / "round-01" / "review.md",
        lane="default-2",
        goal="目标闭集\n`TIMELINE_HANDOFF_TARGETS`决定菜单顺序。",
    )

    prepared = prepare_review_input(review_files=[one_line, wrapped])

    assert "`TIMELINE_HANDOFF_TARGETS`" in prepared.target["locked_goal"]


def test_literal_filenames_with_embedded_backticks_preserve_significant_spaces(tmp_path) -> None:
    # 采纳自 review-pr round-04（lane default-2 #1）：双反引号片段的收尾长度必须与开头一致，
    # 否则它在内部那个单反引号处就被判结束，剩下的半截落回散文、有意义的空格被抹掉。
    first = _pr_review(
        tmp_path / "first" / "round-01" / "review.md",
        lane="first",
        goal="导出产物必须命名为 ``成`片 甲.fcpxml``。",
    )
    second = _pr_review(
        tmp_path / "second" / "round-01" / "review.md",
        lane="second",
        goal="导出产物必须命名为 ``成`片甲.fcpxml``。",
    )

    with pytest.raises(PreparationError, match="locked goal conflict"):
        prepare_review_input(review_files=[first, second])


@pytest.mark.parametrize(("delimiter", "inner_run"), [("`", "``"), ("``", "```")])
def test_longer_backtick_runs_inside_literals_do_not_hide_filename_conflicts(
    tmp_path, delimiter: str, inner_run: str
) -> None:
    # 采纳自 review-pr round-05（lane default-2 #1）：CommonMark 允许 N 个反引号定界的跨度内部
    # 出现长度不等于 N 的 run。收尾判定少了 `(?<!`)` 时，跨度会在那段 run 的后半截提前结束，
    # 剩下的半截落回散文、其中有意义的空格被抹掉。
    first = _pr_review(
        tmp_path / "first" / "round-01" / "review.md",
        lane="first",
        goal=f"导出产物必须命名为 {delimiter}成{inner_run}片 甲.fcpxml{delimiter}。",
    )
    second = _pr_review(
        tmp_path / "second" / "round-01" / "review.md",
        lane="second",
        goal=f"导出产物必须命名为 {delimiter}成{inner_run}片甲.fcpxml{delimiter}。",
    )

    with pytest.raises(PreparationError, match="locked goal conflict"):
        prepare_review_input(review_files=[first, second])


def test_multiline_locked_goal_preserves_fenced_command_requirement(tmp_path) -> None:
    goal = "发布前必须运行以下命令：\n\n```sh\n./run test cli --file cli/tests/test_release.py\n```\n\n其余发布流程保持不变。"
    review = _pr_review(
        tmp_path / "default" / "round-01" / "review.md", lane="default", goal=goal
    )

    prepared = prepare_review_input(review_files=[review])

    assert prepared.target["locked_goal"] == goal


@pytest.mark.parametrize("fence", ["```", "~~~"])
def test_fenced_goal_keeps_literal_header_and_heading_lines(tmp_path, fence) -> None:
    goal = f"保留以下正文：\n{fence}md\n## 目标\n**审查者**：literal  \n{fence}\n结束。"
    review = _pr_review(
        tmp_path / "default" / "round-01" / "review.md", lane="default", goal=goal
    )

    prepared = prepare_review_input(review_files=[review])

    assert prepared.target["locked_goal"] == goal


@pytest.mark.parametrize("fence", ["```", "~~~"])
def test_fenced_goal_filename_differences_remain_conflicts(tmp_path, fence) -> None:
    first = _pr_review(
        tmp_path / "first" / "round-01" / "review.md",
        lane="first",
        goal=f"保存到：\n{fence}text\n成 片.fcpxml\n{fence}\n结束。",
    )
    second = _pr_review(
        tmp_path / "second" / "round-01" / "review.md",
        lane="second",
        goal=f"保存到：\n{fence}text\n成片.fcpxml\n{fence}\n结束。",
    )

    with pytest.raises(PreparationError, match="locked goal conflict"):
        prepare_review_input(review_files=[first, second])


@pytest.mark.parametrize("fence", ["```", "~~~"])
def test_fenced_goal_allows_prose_wrapping_without_changing_payload(tmp_path, fence) -> None:
    goal = f"导出时间线。\n{fence}text\n成 片.fcpxml\n{fence}\n保留帧率。"
    first = _pr_review(
        tmp_path / "first" / "round-01" / "review.md", lane="first", goal=goal
    )
    second = _pr_review(
        tmp_path / "second" / "round-01" / "review.md",
        lane="second",
        goal=goal.replace("导出时间线", "导出\n时间线"),
    )

    prepared = prepare_review_input(review_files=[first, second])

    assert prepared.target["locked_goal"] == goal
