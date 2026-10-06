"""The ledger is the only thing this skill must preserve: no path that writes it may break it.

The whole point of retaining the ranking is an examined fallback after the first place fails. Once the
ledger is overwritten, truncated, or throws a traceback because of its shape, that fallback is gone, and
none of these three surface anywhere else.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest


SCRIPTS_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import main, validate  # noqa: E402
from test_best_of_n import ledger  # noqa: E402


def _write(path: Path, data: dict) -> None:
    path.write_text(json.dumps(data, ensure_ascii=False))


def test_render_output_may_not_overwrite_the_ledger(tmp_path: Path) -> None:
    """The same path would replace the JSON in place with Markdown; the ranking and outcome_log vanish together."""
    path = tmp_path / "ledger.json"
    _write(path, ledger())

    code = main(["record", "--ledger", str(path), "--output", str(path)])

    assert code != 0
    assert path.read_text().lstrip().startswith("{"), "the ledger was overwritten by the rendered output"


@pytest.mark.parametrize(
    "mutate",
    [
        pytest.param(lambda d: d.__setitem__("sources", 1), id="top-level-not-a-list"),
        pytest.param(
            lambda d: d["disputes"][0]["options"][0].__setitem__("sources", 1),
            id="option-not-a-list",
        ),
        pytest.param(
            lambda d: d["disputes"][0]["options"][0].__setitem__("sources", [["x"]]),
            id="option-unhashable-entry",
        ),
    ],
)
def test_malformed_sources_fail_closed_instead_of_raising(mutate) -> None:
    """Fed to set() without validation: non-iterable raises TypeError, unhashable does too; both escape the LedgerError contract."""
    data = ledger()
    mutate(data)

    errors = validate(data)  # must return, not raise

    assert any("sources" in e for e in errors), errors


def test_attempt_writes_atomically(tmp_path: Path) -> None:
    """Writes go through a temporary file + replace, and leave no debris after success."""
    path = tmp_path / "ledger.json"
    _write(path, ledger())

    assert (
        main(
            [
                "attempt",
                "--ledger",
                str(path),
                "--dispute",
                "C3",
                "--option",
                "A",
                "--outcome",
                "still reproduces",
            ]
        )
        == 0
    )

    reloaded = json.loads(path.read_text())
    assert reloaded["disputes"][0]["outcome_log"], "the attempt record was not persisted"
    assert [p.name for p in tmp_path.iterdir() if p.name.startswith(".")] == []
