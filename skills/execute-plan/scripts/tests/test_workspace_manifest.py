"""`git status --porcelain=v1 -z` 的 record 切分契约。

`-z` 下 rename/copy 是**两个** NUL 结尾的字段（`R<X> <new>\0<orig>\0`），而不是一条 record。
按 `\0` 直接 split 会把 `<orig>` 也当成 record，它没有那 3 字节的 `XY ` 前缀，于是取路径的
`record[3:]` 会从路径里啃掉三个字符。
"""

from __future__ import annotations

import sys
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIR))

from workspace_manifest import _porcelain_v1_records  # noqa: E402


def test_rename_yields_the_new_path_not_a_mangled_orig() -> None:
    # 实测 `git mv a.txt b.txt` 的原始输出。旧实现在这里产出第二条 record 且路径为 `xt`。
    records = _porcelain_v1_records(b"R  b.txt\x00a.txt\x00")

    assert [path for _, path in records] == [b"b.txt"]


def test_rename_keeps_the_origin_field_in_the_digest_bytes() -> None:
    # `<orig>` 不该被 stat（它已不在工作树上），但必须留在摘要里——否则「从哪里改名过来」
    # 这一位状态会从 hash 里消失，两次来源不同、目标同名的 rename 会算出同一个 hash。
    from_a = _porcelain_v1_records(b"R  b.txt\x00a.txt\x00")
    from_c = _porcelain_v1_records(b"R  b.txt\x00c.txt\x00")

    assert from_a[0][1] == from_c[0][1] == b"b.txt"
    assert from_a[0][0] != from_c[0][0]


def test_plain_records_are_unchanged_and_copy_behaves_like_rename() -> None:
    plain = _porcelain_v1_records(b" M x.txt\x00?? y.txt\x00")
    assert [path for _, path in plain] == [b"x.txt", b"y.txt"]

    mixed = _porcelain_v1_records(b"R  b.txt\x00a.txt\x00 M x.txt\x00")
    assert [path for _, path in mixed] == [b"b.txt", b"x.txt"]

    copied = _porcelain_v1_records(b"C  d.txt\x00c.txt\x00")
    assert [path for _, path in copied] == [b"d.txt"]


def test_truncated_rename_does_not_consume_past_the_end() -> None:
    # 输出被截断时不得越界读下一条（也不得抛）；只把已有字段当成一条 record。
    records = _porcelain_v1_records(b"R  b.txt\x00")

    assert [path for _, path in records] == [b"b.txt"]
