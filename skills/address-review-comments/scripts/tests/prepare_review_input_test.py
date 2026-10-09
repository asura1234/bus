import os
import sys
from pathlib import Path

import pytest


SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from prepare_review_input import (  # noqa: E402
    PreparationError,
    prepare_review_input,
)


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    # `newline="\n"` is not fussiness: `write_text` defaults to `newline=None`, which translates `\n` into
    # `os.linesep` on write, giving CRLF on Windows. The code under test uses `require_canonical_text_bytes`
    # to correctly reject CRLF, so these fixtures would fail on Windows for a reason unrelated to the assertions.
    # Every fixture in this suite is meant as LF; this helper is the single write point, and new fixtures just go through it.
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
    # free-form has no round identity; forging a round number would leave the provenance judgment without a basis.
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
        "The Windows file system does not accept CR/LF in file names, so this **input** cannot be "
        "constructed on Windows. The behavior under test is itself platform-independent "
        "(`_validate_provenance_path` is a pure string check, run before resolve and reading "
        "the disk); on Windows it is covered by "
        "`test_control_char_source_paths_are_rejected_before_filesystem_access` below"
        " - that one creates no real file, so it runs on every platform."
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
        "The Windows file system does not accept CR/LF in file names, so this **input** cannot be "
        "constructed on Windows. The behavior under test is itself platform-independent "
        "(`_validate_provenance_path` is a pure string check, run before resolve and reading "
        "the disk); on Windows it is covered by "
        "`test_control_char_source_paths_are_rejected_before_filesystem_access` below"
        " - that one creates no real file, so it runs on every platform."
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
        "The Windows file system does not accept CR/LF in file names, so this **input** cannot be "
        "constructed on Windows. The behavior under test is itself platform-independent "
        "(`_validate_provenance_path` is a pure string check, run before resolve and reading "
        "the disk); on Windows it is covered by "
        "`test_control_char_source_paths_are_rejected_before_filesystem_access` below"
        " - that one creates no real file, so it runs on every platform."
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
    """All three control characters are rejected **before touching the file system**.

    This one **creates no real file**, so it runs on every platform: the Windows file system does not
    accept CR/LF in file names, so the cases above that depend on real files cannot be constructed there.
    The `_validate_provenance_path` under test is a pure string check that runs before `resolve()` and
    reading the disk, so this covers exactly the same product logic, and Windows is not left uncovered
    because of the skip.
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
    assert prepared.content.count("**来源**") == 4  # two sections x two sources


def _fixed_headings(content: str) -> list[str]:
    # Judge with the repo's own fence-aware parsing rules rather than a bare startswith:
    # a `## ` inside a fence is not a structural heading.
    sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
    from cli_extensions.review_artifact_parser import outside_fence_lines

    return [line for _, line in outside_fence_lines(content) if line.startswith("## ")]


def test_free_form_headings_cannot_forge_extra_sections(tmp_path) -> None:
    # External reviews often quote this repo's docs verbatim, so fixed headings appearing in the body are reachable input.
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
    # Wrapped header values used to be silently truncated to their first line: the locked goal is the authority for
    # the GOAL & SCOPE GATE, yet most of it was cut off without an error, and single-lane runs were hit too.
    review = _pr_review(tmp_path / "default" / "round-01" / "review.md", lane="default", goal=_WRAPPED_GOAL)

    prepared = prepare_review_input(review_files=[review])

    assert prepared.target["locked_goal"] == _WRAPPED_GOAL
    assert "HandoffTargetAdapter" in prepared.target["locked_goal"]


def test_same_locked_goal_reconciles_across_lanes_that_wrapped_it_differently(tmp_path) -> None:
    # Two lanes each re-serialize the same locked goal: one writes it as a single line, the other wraps it Markdown-style and adds a list.
    # This is only a typesetting difference and should not halt with `locked goal conflict`.
    one_line = _pr_review(tmp_path / "a" / "round-01" / "review.md", lane="default", goal=_ONE_LINE_GOAL)
    wrapped = _pr_review(tmp_path / "b" / "round-01" / "review.md", lane="default-2", goal=_WRAPPED_GOAL)

    prepared = prepare_review_input(review_files=[one_line, wrapped])

    # What gets stored is still the first lane's original text; relaxing the comparison is not rewriting.
    assert prepared.target["locked_goal"] == _ONE_LINE_GOAL


def test_genuinely_different_locked_goals_still_conflict(tmp_path) -> None:
    # The relaxation stops at typesetting: two goals with different wording must still be blocked as before.
    mine = _pr_review(tmp_path / "a" / "round-01" / "review.md", lane="default", goal=_ONE_LINE_GOAL)
    other = _pr_review(
        tmp_path / "b" / "round-01" / "review.md",
        lane="default-2",
        goal="把当前时间线交给外部 NLE，为视频编辑器补上两条产出出口：「在剪映中打开」。",
    )

    with pytest.raises(PreparationError, match="locked goal conflict"):
        prepare_review_input(review_files=[mine, other])


def test_distinct_literal_output_filenames_remain_conflicting_locked_goals(tmp_path) -> None:
    # Probe adopted from review-pr round-03 (lane default-2 #2): whitespace inside backticks is part of the literal,
    # not typesetting. Relaxing this far would silently let through a real conflict like "two lanes locked different output names".
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
    # The literal exemption must not take back the tolerance for prose along with it: same goal, same literal, only the wrap position differs.
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
    # Same nature as the previous one, except the wrap falls **before** the literal instead of after. In the previous one the break is preceded by a full-width comma,
    # which `(?<=[PUNCT])\s+` absorbed; here the break is preceded by an ordinary Han character and followed by a backtick, neither side being CJK
    # nor full-width punctuation, so that newline survived and was counted as a difference.
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
    # Adopted from review-pr round-04 (lane default-2 #1): a double-backtick span's closing length must match its opening,
    # otherwise it is judged closed at the inner single backtick, the remaining half falls back into prose, and the meaningful space is erased.
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
    # Adopted from review-pr round-05 (lane default-2 #1): CommonMark allows a span delimited by N backticks to contain
    # runs whose length is not N. When the closing check lacks `(?<!`)`, the span ends early at the second half of that run,
    # and the remaining half falls back into prose, with its meaningful space erased.
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
